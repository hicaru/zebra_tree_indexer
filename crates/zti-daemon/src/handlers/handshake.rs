use zti_protocol::request::HandshakeReq;
use zti_protocol::response::{HandshakeResp, Response};

pub fn handle(_req: &HandshakeReq) -> Response {
    Response::Handshake(HandshakeResp {
        ok: true,
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        protocol_version: zti_protocol::PROTOCOL_VERSION,
    })
}
