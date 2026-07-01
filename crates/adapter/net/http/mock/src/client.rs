//! A canned-response `Service` leaf that records the requests it receives.

use crate::{MockBody, lock};
use bytes::Bytes;
use oath_adapter_net_http_api::{HttpError, Service};
use std::future::Future;
use std::sync::{Arc, Mutex};

/// A leaf client that returns a fixed status + body and records every request.
#[derive(Debug, Clone)]
pub struct MockClient {
    status: http::StatusCode,
    frames: Vec<Bytes>,
    requests: Arc<Mutex<Vec<http::Request<Bytes>>>>,
}

impl MockClient {
    /// A client returning `status` with a body of `frames`.
    #[must_use]
    pub fn new(status: http::StatusCode, frames: impl IntoIterator<Item = Bytes>) -> Self {
        Self {
            status,
            frames: frames.into_iter().collect(),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// A `200 OK` client whose body is `body`.
    #[must_use]
    pub fn ok(body: impl Into<Bytes>) -> Self {
        Self::new(http::StatusCode::OK, [body.into()])
    }

    /// The requests this client has received, in order.
    #[must_use]
    pub fn recorded_requests(&self) -> Vec<http::Request<Bytes>> {
        lock(&self.requests).clone()
    }
}

impl Service<http::Request<Bytes>> for MockClient {
    type Response = http::Response<MockBody>;
    type Error = HttpError;

    fn call(
        &self,
        req: http::Request<Bytes>,
    ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
        let requests = Arc::clone(&self.requests);
        let status = self.status;
        let frames = self.frames.clone();
        async move {
            lock(&requests).push(req);
            let mut resp = http::Response::new(MockBody::new(frames));
            *resp.status_mut() = status;
            Ok(resp)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MockClient;
    use bytes::Bytes;
    use http_body_util::BodyExt;
    use oath_adapter_net_http_api::HttpClient;

    #[tokio::test]
    async fn returns_canned_body_and_records_requests() {
        let client = MockClient::ok(Bytes::from_static(b"pong"));
        let mut req = http::Request::new(Bytes::from_static(b"ping"));
        *req.uri_mut() = "/tickle".parse().unwrap();
        let resp = client.send(req).await.unwrap();
        assert_eq!(resp.status(), http::StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, Bytes::from_static(b"pong"));
        let recorded = client.recorded_requests();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].uri(), "/tickle");
    }
}
