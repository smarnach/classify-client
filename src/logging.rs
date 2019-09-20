use actix_web::{
    dev::{Service, ServiceRequest, ServiceResponse, Transform},
    Error, HttpRequest, HttpResponse,
};
use futures::{future, Future, Poll};
use slog::{self, Drain};
use slog_async;
use slog_derive::KV;
use slog_mozlog_json::MozLogJson;
use slog_term;
use std::io;

use crate::endpoints::EndpointState;

pub fn get_logger<S: Into<String>>(prefix: S, human_logs: bool) -> slog::Logger {
    let prefix = prefix.into();
    let drain = if human_logs {
        let decorator = slog_term::TermDecorator::new().build();
        let drain = slog_term::CompactFormat::new(decorator).build().fuse();
        slog_async::Async::new(drain).build().fuse()
    } else {
        let drain = MozLogJson::new(io::stdout())
            .logger_name(format!(
                "{}-{}",
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION")
            ))
            .msg_type(prefix)
            .build()
            .fuse();
        slog_async::Async::new(drain).build().fuse()
    };

    slog::Logger::root(drain, slog::o!())
}

#[derive(KV, Default, Debug, Clone)]
struct MozLogFields {
    method: Option<String>,
    path: Option<String>,
    code: Option<u16>,
    agent: Option<String>,
    remote: Option<String>,
    lang: Option<String>,
}

impl MozLogFields {
    fn new<B>(service_response: &ServiceResponse<B>) -> Self {
        Self::default()
            .add_request(service_response.request())
            .add_response(service_response.response())
    }

    fn add_request(mut self, request: &HttpRequest) -> Self {
        self.method = Some(request.method().to_string());
        self.path = Some(request.uri().to_string());

        let headers = request.headers();
        self.agent = headers
            .get("User-Agent")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_string());
        self.lang = headers
            .get("Accept-Language")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_string());
        self.remote = request.connection_info().remote().map(|r| r.to_string());
        self
    }

    fn add_response<B>(mut self, response: &HttpResponse<B>) -> Self {
        self.code = Some(response.status().as_u16());
        self
    }
}

pub struct RequestLogger;

impl<S, B> Transform<S> for RequestLogger
where
    S: Service<Request = ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: 'static,
{
    type Request = ServiceRequest;
    type Response = ServiceResponse<B>;
    type Error = Error;
    type InitError = ();
    type Transform = RequestLoggerMiddleware<S>;
    type Future = future::FutureResult<Self::Transform, Self::InitError>;

    fn new_transform(&self, service: S) -> Self::Future {
        future::ok(RequestLoggerMiddleware { service })
    }
}

pub struct RequestLoggerMiddleware<S> {
    service: S,
}

impl<S, B> Service for RequestLoggerMiddleware<S>
where
    S: Service<Request = ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: 'static,
{
    type Request = ServiceRequest;
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = Box<dyn Future<Item = Self::Response, Error = Self::Error>>;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        self.service.poll_ready()
    }

    fn call(&mut self, req: ServiceRequest) -> Self::Future {
        let log = match req.app_data::<EndpointState>() {
            Some(state) => state.log.clone(),
            None => return Box::new(self.service.call(req)),
        };

        Box::new(self.service.call(req).and_then(move |res| {
            let fields = MozLogFields::new(&res);
            slog::info!(log, "" ; slog::o!(fields));
            Ok(res)
        }))
    }
}

#[cfg(test)]
mod tests {
    use crate::logging::MozLogFields;
    use actix_web::{http, test, HttpResponse};

    #[test]
    fn test_request_fields() {
        let request =
            test::TestRequest::with_header("User-Agent", "test-request").to_http_request();
        let response = HttpResponse::build(http::StatusCode::CREATED).finish();
        let fields = MozLogFields::default()
            .add_request(&request)
            .add_response(&response);

        assert_eq!(fields.method, Some("GET".to_string()));
        assert_eq!(fields.path, Some("/".to_string()));
        assert_eq!(fields.code, Some(201));
        assert_eq!(fields.agent, Some("test-request".into()));
        assert_eq!(fields.lang, None);
        assert_eq!(fields.remote, None);
    }
}
