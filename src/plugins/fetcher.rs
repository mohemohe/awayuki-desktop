use std::cell::RefCell;
use std::future::{self, Future};
use std::rc::Rc;
use std::time::Duration;

use boa_engine::{
    js_string, Context, Finalize, JsData, JsError, JsObject, JsResult, JsString, Trace,
};
use boa_runtime::abort::JsAbortSignal;
use boa_runtime::fetch::request::JsRequest;
use boa_runtime::fetch::response::JsResponse;
use boa_runtime::fetch::Fetcher;

#[cfg(not(test))]
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(test)]
const CONNECT_TIMEOUT: Duration = Duration::from_millis(100);

// This must stay below the plugin callback deadline so a blocking HTTP call
// returns control to the actor before its outer command deadline expires.
#[cfg(not(test))]
pub(super) const TOTAL_TIMEOUT: Duration = Duration::from_secs(25);
#[cfg(test)]
pub(super) const TOTAL_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Clone, Debug)]
pub(super) struct BoundedReqwestFetcher {
    client: reqwest::blocking::Client,
}

impl BoundedReqwestFetcher {
    pub(super) fn new() -> Result<Self, reqwest::Error> {
        reqwest::blocking::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            // reqwest's client timeout covers connection, redirects, response
            // headers, and reading the response body.
            .timeout(TOTAL_TIMEOUT)
            .build()
            .map(|client| Self { client })
    }
}

impl Finalize for BoundedReqwestFetcher {}

// SAFETY: reqwest's client contains no Boa GC handles.
unsafe impl Trace for BoundedReqwestFetcher {
    unsafe fn trace(&self, _tracer: &mut boa_engine::gc::Tracer) {}

    unsafe fn trace_non_roots(&self) {}

    fn run_finalizer(&self) {
        Finalize::finalize(self);
    }
}

impl JsData for BoundedReqwestFetcher {}

impl Fetcher for BoundedReqwestFetcher {
    fn fetch(
        self: Rc<Self>,
        request: JsRequest,
        signal: Option<JsObject>,
        _context: &RefCell<&mut Context>,
    ) -> impl Future<Output = JsResult<JsResponse>> {
        if is_aborted(signal.as_ref()) {
            return future::ready(Err(abort_error()));
        }

        let request = request.into_inner();
        let url = request.uri().to_string();
        let request = self
            .client
            .request(request.method().clone(), &url)
            .headers(request.headers().clone())
            .body(request.body().clone())
            .build()
            .map_err(reqwest_error);
        let request = match request {
            Ok(request) => request,
            Err(error) => return future::ready(Err(error)),
        };

        let response = match self.client.execute(request).map_err(reqwest_error) {
            Ok(response) => response,
            Err(error) => return future::ready(Err(error)),
        };
        if is_aborted(signal.as_ref()) {
            return future::ready(Err(abort_error()));
        }

        let status = response.status();
        let headers = response.headers().clone();
        let body = match response.bytes().map_err(reqwest_error) {
            Ok(body) => body,
            Err(error) => return future::ready(Err(error)),
        };
        let mut builder = http::Response::builder().status(status.as_u16());
        for name in headers.keys() {
            for value in headers.get_all(name) {
                builder = builder.header(name.as_str(), value);
            }
        }

        future::ready(
            builder
                .body(body.to_vec())
                .map_err(JsError::from_rust)
                .map(|response| JsResponse::basic(JsString::from(url), response)),
        )
    }
}

fn is_aborted(signal: Option<&JsObject>) -> bool {
    signal
        .and_then(|signal| signal.downcast_ref::<JsAbortSignal>())
        .is_some_and(|signal| signal.is_aborted())
}

fn abort_error() -> JsError {
    JsError::from_opaque(js_string!("AbortError").into())
}

fn reqwest_error(error: reqwest::Error) -> JsError {
    if error.is_timeout() {
        JsError::from_opaque(js_string!("fetch timed out").into())
    } else {
        JsError::from_rust(error)
    }
}
