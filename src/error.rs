#[derive(Debug)]
pub enum RaknetError{
    SetRaknetRawSocketError,
    NotListen,
    BindAdreesError,
    ConnectionClosed,
    NotSupportVersion,
    IncorrectReply,
    PacketParseError,
    RecvFromError,
    IncorrectReliability,
    IncorrectPacketID,
    ReadPacketBufferError,
    PacketSizeExceedMTU
}

pub type Result<T> = std::result::Result<T , RaknetError>;