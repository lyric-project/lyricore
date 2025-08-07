use crate::path::ActorAddress;
use crate::serialization::MessageEnvelope;
use tokio::sync::oneshot;

pub(crate) enum InboxMessage {
    EnvelopeMessage {
        addr: ActorAddress,
        envelope: MessageEnvelope,
    },
    RpcEnvelopeMessage {
        addr: ActorAddress,
        envelope: MessageEnvelope,
        response_tx: oneshot::Sender<crate::error::Result<MessageEnvelope>>,
    },
    OnStart,
    OnStop,
    RemoteProcessConnected {
        addr: ActorAddress,
    },
    RemoteProcessDisconnected {
        addr: ActorAddress,
    },
}

impl InboxMessage {
    pub fn envelope_message(addr: ActorAddress, envelope: MessageEnvelope) -> Self {
        InboxMessage::EnvelopeMessage { addr, envelope }
    }

    pub fn rpc_envelope_message(
        addr: ActorAddress,
        envelope: MessageEnvelope,
        response_tx: oneshot::Sender<crate::error::Result<MessageEnvelope>>,
    ) -> Self {
        InboxMessage::RpcEnvelopeMessage {
            addr,
            envelope,
            response_tx,
        }
    }
}
