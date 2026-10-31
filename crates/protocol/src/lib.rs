//! Native protocol engines used by the desktop workbench.

mod grpc;
mod realtime;

pub use grpc::{
    GrpcInvokeInput, GrpcInvokeResult, GrpcMethodInfo, GrpcReflectionInput, GrpcReflectionResult,
    invoke_grpc_unary, reflect_grpc_service,
};
pub use realtime::{
    ProtocolEvent, ProtocolEventDirection, ProtocolEventKind, ProtocolSession,
    ServerSentEventsInput, WebSocketInput, WebSocketOutbound, start_server_sent_events,
    start_websocket,
};
