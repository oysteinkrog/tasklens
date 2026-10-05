//! Sockets: sock_diag dump, socket to process map, TCP attribution, connection records.
pub mod attribution;
pub mod connections;
pub mod sock_diag;
pub mod socket_map;

pub use attribution::{NetRate, TcpAttribution};
pub use connections::{ConnCounts, ConnRecord, ConnectionTable, IfNames, Owner, format_endpoint};
pub use sock_diag::{
    Family, Proto, SockDiag, SockRecord, TcpInfoLite, tcp_state_label, udp_state_label,
};
pub use socket_map::SocketMap;
