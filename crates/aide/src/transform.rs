//! Transformers wrap a part of or the entirety of [`OpenApi`]
//! and enable modifications with method chaining in a declarative way.
//!
//! # Examples
//!
//! Example documentation for an imaginary government-provided API:
//!
//! ```ignore
//! op.description("An example operation.")
//!     .response_with::<200, Json<String>, _>(|res| {
//!         res.description(
//!             "Something was probably successful, we don't know \
//!                     what this returns, but it's at least JSON.",
//!         )
//!     })
//!     .response_with::<500, Html<String>, _>(|res| {
//!         res.description("Sometimes arbitrary 500 is returned with randomized HTML.")
//!     })
//!     .default_response::<String>()
//! ```
//!
//! # Transform Functions
//!
//! Transform functions are functions that accept a single
//! transformer as a parameter and return it. They
//! enable composability of documentation.
//!
//! ## Example
//!
//! ```rust
//! # use aide::transform::TransformOperation;
//! /// This transform function simply adds a no-content response as an example.
//! fn no_content(op: TransformOperation) -> TransformOperation {
//!     op.response::<204, ()>()
//! }
//! ```
//!
//! The above then can be (re)used using `with`.
//!
//! ```
//! # use aide::transform::TransformOperation;
//! # fn no_content(op: TransformOperation) -> TransformOperation {
//!     op.description("this operation always returns nothing")
//!         .with(no_content)
//! # }
//! ```
//!

use std::{any::type_name, marker::PhantomData};

use crate::{
    gen::GenContext,
    openapi::{OpenApi, Operation, Parameter, PathItem, ReferenceOr, Response, StatusCode},
};
use serde::Serialize;

use crate::{error::Error, gen::in_context, operation::OperationOutput, util::iter_operations_mut};

/// A transform helper that wraps [`OpenApi`].
#[must_use]
pub struct TransformOpenApi<'t> {
    pub(crate) api: &'t mut OpenApi,
}

impl<'t> TransformOpenApi<'t> {
    /// Create a new transform helper.
    pub fn new(api: &'t mut OpenApi) -> Self {
        Self { api }
    }

    /// Set a default response for all operations
    /// that do not already have one.
    #[tracing::instrument(skip_all)]
    pub fn default_response<R>(self) -> Self
    where
        R: OperationOutput,
    {
        if let Some(p) = &mut self.api.paths {
            for (_, p) in &mut p.paths {
                let p = match p {
                    ReferenceOr::Reference { .. } => continue,
                    ReferenceOr::Item(p) => p,
                };

                let _ = TransformPathItem::new(p).default_response::<R>();
            }
        }

        self
    }

    /// Set a default response for all operations
    /// that do not already have one.
    ///
    /// This method additionally accepts a transform function
    /// to modify the generated documentation.
    #[tracing::instrument(skip_all)]
    pub fn default_response_with<R, F>(self, transform: F) -> Self
    where
        R: OperationOutput,
        F: Fn(TransformResponse<R::Inner>) -> TransformResponse<R::Inner> + Clone,
    {
        if let Some(p) = &mut self.api.paths {
            for (_, p) in &mut p.paths {
                let p = match p {
                    ReferenceOr::Reference { .. } => continue,
                    ReferenceOr::Item(p) => p,
                };

                for (_, op) in iter_operations_mut(p) {
                    let _ = TransformOperation::new(op)
                        .default_response_with::<R, F>(transform.clone());
                }
            }
        }

        self
    }

    /// Apply an another transform function.
    pub fn with(self, transform: impl FnOnce(Self) -> Self) -> Self {
        transform(self)
    }

    /// Access the inner [`OpenApi`].
    #[inline]
    pub fn inner_mut(&mut self) -> &mut OpenApi {
        self.api
    }
}

/// A transform helper that wraps [`TransformPathItem`].
#[must_use]
pub struct TransformPathItem<'t> {
    pub(crate) hidden: bool,
    pub(crate) path: &'t mut PathItem,
}

impl<'t> TransformPathItem<'t> {
    /// Create a new transform helper.
    pub fn new(path: &'t mut PathItem) -> Self {
        Self {
            hidden: false,
            path,
        }
    }

    /// Hide the path from the documentation.
    ///
    /// This is taken into account by generators provided
    /// by this library.
    ///
    /// Hiding an item causes it to be ignored
    /// completely, there is no way to restore or "unhide" it afterwards.
    #[tracing::instrument(skip_all)]
    pub fn hidden(mut self, hidden: bool) -> Self {
        self.hidden = hidden;
        self
    }

    /// Provide a description for the path.
    #[tracing::instrument(skip_all)]
    pub fn description(mut self, desc: &str) -> Self {
        self.path.description = Some(desc.into());
        self
    }

    /// Set a default response for all operations in the
    /// path that do not already have one.
    #[tracing::instrument(skip_all)]
    pub fn default_response<R>(self) -> Self
    where
        R: OperationOutput,
    {
        in_context(|ctx| ctx.show_error = filter_no_duplicate_response);

        for (_, op) in iter_operations_mut(self.path) {
            let _ = TransformOperation::new(op).default_response::<R>();
        }

        in_context(GenContext::reset_error_filter);

        self
    }

    /// Set a default response for all operations in the
    /// path that do not already have one.
    ///
    /// This method additionally accepts a transform function
    /// to modify the generated documentation.
    #[tracing::instrument(skip_all)]
    pub fn default_response_with<R, F>(self, transform: F) -> Self
    where
        R: OperationOutput,
        F: Fn(TransformResponse<R::Inner>) -> TransformResponse<R::Inner> + Clone,
    {
        in_context(|ctx| ctx.show_error = filter_no_duplicate_response);

        for (_, op) in iter_operations_mut(self.path) {
            let _ = TransformOperation::new(op).default_response_with::<R, F>(transform.clone());
        }

        in_context(GenContext::reset_error_filter);

        self
    }

    /// Apply an another transform function.
    pub fn with(self, transform: impl FnOnce(Self) -> Self) -> Self {
        transform(self)
    }

    /// Access the inner [`PathItem`].
    #[inline]
    pub fn inner_mut(&mut self) -> &mut PathItem {
        self.path
    }
}

/// A transform helper that wraps [`Operation`].
#[must_use]
pub struct TransformOperation<'t> {
    pub(crate) hidden: bool,
    pub(crate) operation: &'t mut Operation,
}

impl<'t> TransformOperation<'t> {
    /// Create a new transform helper.
    pub fn new(operation: &'t mut Operation) -> Self {
        Self {
            hidden: false,
            operation,
        }
    }

    /// Specify the operation ID.
    #[tracing::instrument(skip_all, fields(operation_id = ?self.operation.operation_id))]
    pub fn id(mut self, name: &str) -> Self {
        self.operation.operation_id = Some(name.into());
        self
    }

    /// Provide a description for the operation.
    #[tracing::instrument(skip_all, fields(operation_id = ?self.operation.operation_id))]
    pub fn description(mut self, desc: &str) -> Self {
        self.operation.description = Some(desc.into());
        self
    }

    /// Hide the operation from the documentation.
    ///
    /// This is taken into account by generators provided
    /// by this library.
    ///
    /// Hiding an item causes it to be ignored
    /// completely, there is no way to restore or "unhide" it afterwards.
    #[tracing::instrument(skip_all, fields(operation_id = ?self.operation.operation_id))]
    pub fn hidden(mut self, hidden: bool) -> Self {
        self.hidden = hidden;
        self
    }

    /// Modify a parameter of the operation.
    #[tracing::instrument(skip_all, fields(operation_id = ?self.operation.operation_id))]
    pub fn parameter<T, F>(self, name: &str, transform: F) -> Self
    where
        T: Serialize,
        F: FnOnce(TransformParameter<T>) -> TransformParameter<T>,
    {
        let (idx, param) = match self
            .operation
            .parameters
            .iter_mut()
            .enumerate()
            .find(|(_, p)| match p {
                ReferenceOr::Item(p) => p.parameter_data_ref().name == name,
                ReferenceOr::Reference { .. } => false,
            }) {
            Some((idx, p)) => match p {
                ReferenceOr::Item(p) => (idx, p),
                ReferenceOr::Reference { .. } => unreachable!(),
            },
            None => {
                in_context(|ctx| {
                    ctx.error(Error::ParameterNotExists(name.to_string()));
                });
                return self;
            }
        };

        let t = transform(TransformParameter::new(param));

        if t.hidden {
            self.operation.parameters.remove(idx);
        }

        self
    }

    /// Modify a parameter of the operation without knowing a type.
    ///
    /// The type `()` will be used instead.
    #[tracing::instrument(skip_all, fields(operation_id = ?self.operation.operation_id))]
    pub fn parameter_untyped<F>(self, name: &str, transform: F) -> Self
    where
        F: FnOnce(TransformParameter<()>) -> TransformParameter<()>,
    {
        self.parameter(name, transform)
    }

    /// Set a default response for the operation if
    /// it does not already have one.
    #[tracing::instrument(skip_all, fields(operation_id = ?self.operation.operation_id))]
    #[allow(clippy::missing_panics_doc)]
    pub fn default_response<R>(mut self) -> Self
    where
        R: OperationOutput,
    {
        if self.operation.responses.is_none() {
            self.operation.responses = Some(Default::default());
        }

        in_context(|ctx| {
            if let Some(res) = R::operation_response(ctx, self.operation) {
                let responses = self.operation.responses.as_mut().unwrap();
                if responses.default.is_none() {
                    responses.default = Some(ReferenceOr::Item(res));
                } else {
                    ctx.error(Error::DefaultResponseExists);
                }
            } else {
                tracing::debug!(type_name = type_name::<R>(), "no response info of type");
            }
        });

        self
    }

    /// Set a default response for the operation if
    /// it does not already have one.
    ///
    /// This method additionally accepts a transform function
    /// to modify the generated documentation.
    #[tracing::instrument(skip_all, fields(operation_id = ?self.operation.operation_id))]
    #[allow(clippy::missing_panics_doc)]
    pub fn default_response_with<R, F>(mut self, transform: F) -> Self
    where
        R: OperationOutput,
        F: FnOnce(TransformResponse<R::Inner>) -> TransformResponse<R::Inner>,
    {
        if self.operation.responses.is_none() {
            self.operation.responses = Some(Default::default());
        } else {
            tracing::trace!("operation already has a default response");
        }

        in_context(|ctx| {
            if let Some(mut res) = R::operation_response(ctx, self.operation) {
                let responses = self.operation.responses.as_mut().unwrap();
                if responses.default.is_none() {
                    let t = transform(TransformResponse::new(&mut res));

                    if !t.hidden {
                        responses.default = Some(ReferenceOr::Item(res));
                    }
                } else {
                    ctx.error(Error::DefaultResponseExists);
                }
            } else {
                tracing::debug!(type_name = type_name::<R>(), "no response info of type");
            }
        });

        self
    }

    /// Add a response to the operation with the given status code.
    #[tracing::instrument(skip_all, fields(operation_id = ?self.operation.operation_id))]
    #[allow(clippy::missing_panics_doc)]
    pub fn response<const N: u16, R>(mut self) -> Self
    where
        R: OperationOutput,
    {
        if self.operation.responses.is_none() {
            self.operation.responses = Some(Default::default());
        }

        in_context(|ctx| {
            if let Some(res) = R::operation_response(ctx, self.operation) {
                let responses = self.operation.responses.as_mut().unwrap();
                if responses
                    .responses
                    .insert(StatusCode::Code(N), ReferenceOr::Item(res))
                    .is_some()
                {
                    ctx.error(Error::ResponseExists(StatusCode::Code(N)));
                };
            } else {
                tracing::debug!(type_name = type_name::<R>(), "no response info of type");
            }
        });

        self
    }

    /// Add a response to the operation with the given status code.
    ///
    /// This method additionally accepts a transform function
    /// to modify the generated documentation.
    #[tracing::instrument(skip_all, fields(operation_id = ?self.operation.operation_id))]
    #[allow(clippy::missing_panics_doc)]
    pub fn response_with<const N: u16, R, F>(mut self, transform: F) -> Self
    where
        R: OperationOutput,
        F: FnOnce(TransformResponse<R::Inner>) -> TransformResponse<R::Inner>,
    {
        if self.operation.responses.is_none() {
            self.operation.responses = Some(Default::default());
        }

        in_context(|ctx| {
            if let Some(mut res) = R::operation_response(ctx, self.operation) {
                let t = transform(TransformResponse::new(&mut res));

                let responses = self.operation.responses.as_mut().unwrap();
                if !t.hidden {
                    let existing = responses
                        .responses
                        .insert(StatusCode::Code(N), ReferenceOr::Item(res))
                        .is_some();
                    if existing {
                        ctx.error(Error::ResponseExists(StatusCode::Code(N)));
                    };
                }
            } else {
                tracing::debug!(type_name = type_name::<R>(), "no response info of type");
            }
        });

        self
    }

    /// Add a response to the operation with the given status code range (e.g. 2xx).
    ///
    /// Note that the range is `100`-based, so for the range `2xx`, `2` must be provided.
    #[tracing::instrument(skip_all, fields(operation_id = ?self.operation.operation_id))]
    #[allow(clippy::missing_panics_doc)]
    pub fn response_range<const N: u16, R>(mut self) -> Self
    where
        R: OperationOutput,
    {
        if self.operation.responses.is_none() {
            self.operation.responses = Some(Default::default());
        }

        in_context(|ctx| {
            if let Some(res) = R::operation_response(ctx, self.operation) {
                let responses = self.operation.responses.as_mut().unwrap();
                if responses
                    .responses
                    .insert(StatusCode::Code(N), ReferenceOr::Item(res))
                    .is_some()
                {
                    ctx.error(Error::ResponseExists(StatusCode::Range(N)));
                };
            } else {
                tracing::debug!(type_name = type_name::<R>(), "no response info of type");
            }
        });

        self
    }

    /// Add a response to the operation with the given status code range (e.g. 2xx).
    ///
    /// Note that the range is `100`-based, so for the range `2xx`, `2` must be provided.
    ///
    /// This method additionally accepts a transform function
    /// to modify the generated documentation.
    #[tracing::instrument(skip_all, fields(operation_id = ?self.operation.operation_id))]
    #[allow(clippy::missing_panics_doc)]
    pub fn response_range_with<const N: u16, R, F>(mut self, transform: F) -> Self
    where
        R: OperationOutput,
        F: FnOnce(TransformResponse<R::Inner>) -> TransformResponse<R::Inner>,
    {
        if self.operation.responses.is_none() {
            self.operation.responses = Some(Default::default());
        }

        in_context(|ctx| {
            if let Some(mut res) = R::operation_response(ctx, self.operation) {
                let t = transform(TransformResponse::new(&mut res));

                let responses = self.operation.responses.as_mut().unwrap();
                if !t.hidden {
                    let existing = responses
                        .responses
                        .insert(StatusCode::Range(N), ReferenceOr::Item(res))
                        .is_some();
                    if existing {
                        ctx.error(Error::ResponseExists(StatusCode::Range(N)));
                    };
                }
            } else {
                tracing::debug!(type_name = type_name::<R>(), "no response info of type");
            }
        });

        self
    }

    /// Apply an another transform function.
    pub fn with(self, transform: impl FnOnce(Self) -> Self) -> Self {
        transform(self)
    }

    /// Access the inner [`Operation`].
    #[inline]
    pub fn inner_mut(&mut self) -> &mut Operation {
        self.operation
    }
}

/// A transform helper that wraps [`Parameter`].
///
/// An additional type is provided for strongly-typed
/// examples.
#[must_use]
pub struct TransformParameter<'t, T> {
    pub(crate) hidden: bool,
    pub(crate) param: &'t mut Parameter,
    _t: PhantomData<T>,
}

impl<'t, T> TransformParameter<'t, T> {
    /// Create a new transform helper.
    pub fn new(param: &'t mut Parameter) -> Self {
        Self {
            hidden: false,
            param,
            _t: PhantomData,
        }
    }

    /// Hide the parameter from the documentation.
    ///
    /// This is taken into account by generators provided
    /// by this library.
    ///
    /// Hiding an item causes it to be ignored
    /// completely, there is no way to restore or "unhide" it afterwards.
    #[tracing::instrument(skip_all)]
    pub fn hidden(mut self, hidden: bool) -> Self {
        self.hidden = hidden;
        self
    }

    /// Provide or override the description of the parameter.
    #[tracing::instrument(skip_all)]
    pub fn description(mut self, desc: &str) -> Self {
        let data = match &mut self.param {
            Parameter::Query { parameter_data, .. }
            | Parameter::Header { parameter_data, .. }
            | Parameter::Path { parameter_data, .. }
            | Parameter::Cookie { parameter_data, .. } => parameter_data,
        };
        data.description = Some(desc.into());
        self
    }

    /// Apply an another transform function.
    pub fn with(self, transform: impl FnOnce(Self) -> Self) -> Self {
        transform(self)
    }

    /// Access the inner [`Parameter`].
    #[inline]
    pub fn inner_mut(&mut self) -> &mut Parameter {
        self.param
    }
}

/// A transform helper that wraps [`Response`].
///
/// An additional type is provided for strongly-typed
/// examples.
#[must_use]
pub struct TransformResponse<'t, T> {
    pub(crate) hidden: bool,
    pub(crate) response: &'t mut Response,
    _t: PhantomData<T>,
}

impl<'t, T> TransformResponse<'t, T> {
    /// Create a new transform helper.
    pub fn new(response: &'t mut Response) -> Self {
        Self {
            hidden: false,
            response,
            _t: PhantomData,
        }
    }

    /// Hide the response from the documentation.
    ///
    /// This is taken into account by generators provided
    /// by this library.
    ///
    /// Hiding an item causes it to be ignored
    /// completely, there is no way to restore or "unhide" it afterwards.
    #[tracing::instrument(skip_all)]
    pub fn hidden(mut self, hidden: bool) -> Self {
        self.hidden = hidden;
        self
    }

    /// Provide or override the description of the response.
    #[tracing::instrument(skip_all)]
    pub fn description(mut self, desc: &str) -> Self {
        self.response.description = desc.into();
        self
    }

    /// Provide or override an example for the response.
    #[tracing::instrument(skip_all)]
    #[allow(clippy::missing_panics_doc)]
    pub fn example(self, example: impl Into<T>) -> Self
    where
        T: Serialize,
    {
        let example = example.into();

        for (_, c) in &mut self.response.content {
            c.example = Some(serde_json::to_value(&example).unwrap());
        }

        self
    }

    /// Apply an another transform function.
    pub fn with(self, transform: impl FnOnce(Self) -> Self) -> Self {
        transform(self)
    }

    /// Access the inner [`Response`].
    pub fn inner(&mut self) -> &mut Response {
        self.response
    }
}

fn filter_no_duplicate_response(err: &Error) -> bool {
    !matches!(err, Error::DefaultResponseExists | Error::ResponseExists(_))
}
