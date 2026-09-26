#[derive(Debug)]
pub enum RaknetError {
    SetRaknetRawSocketError,
    NotListen,
    BindAddressError,
    ConnectionClosed,
    NotSupportVersion,
    IncorrectReply,
    PacketParseError,
    SocketError,
    IncorrectReliability,
    IncorrectPacketID,
    ReadPacketBufferError,
    PacketSizeExceedMTU,
    PacketHeaderError,
    SetMotdError,
}

pub type Result<T> = std::result::Result<T, RaknetError>;

impl std::fmt::Display for RaknetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::SetRaknetRawSocketError => {
                "failed to initialize RakNet from the supplied UDP socket"
            }
            Self::NotListen => "the RakNet listener is not listening",
            Self::BindAddressError => "failed to bind the UDP socket",
            Self::ConnectionClosed => "the RakNet connection is closed",
            Self::NotSupportVersion => "the RakNet protocol version is not supported",
            Self::IncorrectReply => "received an incorrect RakNet reply",
            Self::PacketParseError => "failed to parse a RakNet packet",
            Self::SocketError => "a UDP socket operation failed",
            Self::IncorrectReliability => "the packet reliability is invalid",
            Self::IncorrectPacketID => "the RakNet packet identifier is invalid",
            Self::ReadPacketBufferError => "the packet buffer ended unexpectedly",
            Self::PacketSizeExceedMTU => "the packet exceeds the connection MTU",
            Self::PacketHeaderError => "the packet has an invalid header",
            Self::SetMotdError => "failed to update the server MOTD",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for RaknetError {}
