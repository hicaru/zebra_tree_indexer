// Wire protocol. Every connection MUST send `Request::Handshake` as its first
// frame and consume the matching `Response::Handshake` before issuing any
// other request — `Stop` included.
pub mod codec;
pub mod request;
pub mod response;

pub use request::Request;
pub use response::Response;
