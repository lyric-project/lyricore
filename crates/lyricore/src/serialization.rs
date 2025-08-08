use crate::actor::Message;
use crate::error::{LyricoreActorError, Result};
use crate::rpc::actor_service;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SerFormat {
    Json = 0,
    Protobuf = 1,
    Messagepack = 2,
    Custom = 255,
}

impl From<SerFormat> for i32 {
    fn from(format: SerFormat) -> Self {
        format as i32
    }
}

impl TryFrom<i32> for SerFormat {
    type Error = LyricoreActorError;

    fn try_from(value: i32) -> Result<Self> {
        match value {
            0 => Ok(SerFormat::Json),
            1 => Ok(SerFormat::Protobuf),
            2 => Ok(SerFormat::Messagepack),
            255 => Ok(SerFormat::Custom),
            _ => Err(LyricoreActorError::Actor(
                crate::error::ActorError::RpcError(format!(
                    "Unknown serialization format: {}",
                    value
                )),
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub enum MessageSerializer {
    Json(JsonSerializer),
    MessagePack(MessagePackSerializer),
}

impl MessageSerializer {
    #[inline(always)]
    pub fn serialize<T: Serialize>(&self, value: &T) -> Result<Vec<u8>> {
        match self {
            MessageSerializer::Json(s) => s.serialize(value),
            MessageSerializer::MessagePack(s) => s.serialize(value),
        }
    }
    #[inline(always)]
    pub fn deserialize<T: for<'de> Deserialize<'de>>(&self, data: &[u8]) -> Result<T> {
        match self {
            MessageSerializer::Json(s) => s.deserialize(data),
            MessageSerializer::MessagePack(s) => s.deserialize(data),
        }
    }
    #[inline(always)]
    pub fn format(&self) -> SerFormat {
        match self {
            MessageSerializer::Json(s) => s.format(),
            MessageSerializer::MessagePack(s) => s.format(),
        }
    }

    pub fn supports_type(&self, type_name: &str) -> bool {
        match self {
            MessageSerializer::Json(s) => s.supports_type(type_name),
            MessageSerializer::MessagePack(s) => s.supports_type(type_name),
        }
    }
}

#[derive(Clone, Debug)]
pub struct JsonSerializer;

impl JsonSerializer {
    #[inline(always)]
    pub fn serialize<T: Serialize>(&self, value: &T) -> Result<Vec<u8>> {
        serde_json::to_vec(value).map_err(|e| {
            LyricoreActorError::Actor(crate::error::ActorError::RpcError(e.to_string()))
        })
    }
    #[inline(always)]
    pub fn deserialize<T: for<'de> Deserialize<'de>>(&self, data: &[u8]) -> Result<T> {
        serde_json::from_slice(data).map_err(|e| {
            LyricoreActorError::Actor(crate::error::ActorError::RpcError(e.to_string()))
        })
    }

    pub fn format(&self) -> SerFormat {
        SerFormat::Json
    }

    pub fn supports_type(&self, _type_name: &str) -> bool {
        true
    }
}

// MessagePack Serializer
#[derive(Clone, Debug)]
pub struct MessagePackSerializer;

impl MessagePackSerializer {
    #[inline(always)]
    pub fn serialize<T: Serialize>(&self, value: &T) -> Result<Vec<u8>> {
        rmp_serde::to_vec(value).map_err(|e| {
            LyricoreActorError::Actor(crate::error::ActorError::RpcError(e.to_string()))
        })
    }
    #[inline(always)]
    pub fn deserialize<T: for<'de> Deserialize<'de>>(&self, data: &[u8]) -> Result<T> {
        rmp_serde::from_slice(data).map_err(|e| {
            LyricoreActorError::Actor(crate::error::ActorError::RpcError(e.to_string()))
        })
    }

    pub fn format(&self) -> SerFormat {
        SerFormat::Messagepack
    }

    pub fn supports_type(&self, _type_name: &str) -> bool {
        true
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MessageEnvelope {
    pub message_type: String,
    pub format: SerFormat,
    pub schema_version: u32,
    pub payload: Vec<u8>,
    // pub metadata: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum: Option<Vec<u8>>,
}

impl MessageEnvelope {
    #[inline]
    pub fn new<T: Message + Serialize>(
        message: &T,
        serializer: &MessageSerializer,
    ) -> Result<Self> {
        let payload = serializer.serialize(message)?;
        let message_type = format!(
            "{}::{}",
            Self::get_type_name_cached::<T>(),
            T::SCHEMA_VERSION
        );

        Ok(Self {
            message_type,
            format: serializer.format(),
            schema_version: T::SCHEMA_VERSION,
            payload,
            metadata: None,
            checksum: None,
        })
    }

    fn get_type_name_cached<T: 'static>() -> String {
        use std::collections::HashMap;
        use std::sync::OnceLock;

        static TYPE_NAME_CACHE: OnceLock<std::sync::RwLock<HashMap<std::any::TypeId, String>>> =
            OnceLock::new();

        let type_id = std::any::TypeId::of::<T>();

        let cache = TYPE_NAME_CACHE.get_or_init(|| std::sync::RwLock::new(HashMap::new()));

        // Try to read from the cache first
        if let Ok(read_guard) = cache.read() {
            if let Some(cached_name) = read_guard.get(&type_id) {
                return cached_name.clone();
            }
        }

        // If not cached, acquire a write lock and insert
        let type_name = std::any::type_name::<T>().to_string();
        if let Ok(mut write_guard) = cache.write() {
            write_guard.insert(type_id, type_name.clone());
        }

        type_name
    }

    #[inline]
    pub fn deserialize<T: Message + for<'de> Deserialize<'de>>(
        &self,
        registry: &MessageRegistry,
    ) -> Result<T> {
        let serializer = registry.get_serializer(self.format)?;
        serializer.deserialize(&self.payload)
    }

    pub fn generate_message_type_id<T: Message>() -> String {
        format!(
            "{}::{}",
            Self::get_type_name_cached::<T>(),
            T::SCHEMA_VERSION
        )
    }

    #[inline]
    pub fn check_message_type<T: Message>(&self) -> bool {
        let expected = format!(
            "{}::{}",
            Self::get_type_name_cached::<T>(),
            T::SCHEMA_VERSION
        );
        self.message_type == expected
    }

    pub fn with_metadata(mut self, key: String, value: String) -> Self {
        if self.metadata.is_none() {
            self.metadata = Some(HashMap::new());
        }
        self.metadata.as_mut().unwrap().insert(key, value);
        self
    }
}

// To proto type conversion
impl From<MessageEnvelope> for actor_service::MessageEnvelope {
    fn from(envelope: MessageEnvelope) -> Self {
        Self {
            message_type: envelope.message_type,
            format: envelope.format as i32,
            schema_version: envelope.schema_version,
            payload: envelope.payload,
            metadata: envelope.metadata.unwrap_or_default(),
            checksum: envelope.checksum,
        }
    }
}

// From proto type conversion
impl TryFrom<actor_service::MessageEnvelope> for MessageEnvelope {
    type Error = LyricoreActorError;

    fn try_from(proto: actor_service::MessageEnvelope) -> Result<Self> {
        let format = SerFormat::try_from(proto.format)?;

        Ok(Self {
            message_type: proto.message_type,
            format,
            schema_version: proto.schema_version,
            payload: proto.payload,
            metadata: if proto.metadata.is_empty() {
                None
            } else {
                Some(proto.metadata)
            },
            checksum: proto.checksum,
        })
    }
}

// The serialization strategy defines how messages are serialized and deserialized
#[derive(Clone, Debug)]
pub struct SerializationStrategy {
    pub local_preferred: SerFormat,
    pub remote_fallback: Vec<SerFormat>,
    pub cross_language_default: SerFormat,
}

impl Default for SerializationStrategy {
    fn default() -> Self {
        Self {
            local_preferred: SerFormat::Messagepack,
            remote_fallback: vec![SerFormat::Messagepack, SerFormat::Json],
            cross_language_default: SerFormat::Json,
        }
    }
}

impl SerializationStrategy {
    pub fn fast_json() -> Self {
        Self {
            local_preferred: SerFormat::Json,
            remote_fallback: vec![SerFormat::Json],
            cross_language_default: SerFormat::Json,
        }
    }

    pub fn messagepack() -> Self {
        Self {
            local_preferred: SerFormat::Messagepack,
            remote_fallback: vec![SerFormat::Messagepack, SerFormat::Json],
            cross_language_default: SerFormat::Json,
        }
    }
    pub fn fast_local() -> Self {
        Self::fast_json() // Use json as the local preferred format
    }
}
#[derive(Debug)]
pub struct MessageRegistry {
    serializers: HashMap<SerFormat, MessageSerializer>,
    type_mappings: HashMap<String, SerFormat>,
    schema_versions: HashMap<String, u32>,
    strategy: SerializationStrategy,
    json_serializer: MessageSerializer,
    messagepack_serializer: MessageSerializer,
}

impl MessageRegistry {
    pub fn new(strategy: SerializationStrategy) -> Self {
        let mut registry = Self {
            serializers: HashMap::new(),
            type_mappings: HashMap::new(),
            schema_versions: HashMap::new(),
            json_serializer: MessageSerializer::Json(JsonSerializer),
            messagepack_serializer: MessageSerializer::MessagePack(MessagePackSerializer),
            strategy,
        };

        // Register default serializers
        registry.register_serializer(MessageSerializer::Json(JsonSerializer));
        registry.register_serializer(MessageSerializer::MessagePack(MessagePackSerializer));

        registry
    }
    #[inline]
    pub fn register_serializer(&mut self, serializer: MessageSerializer) {
        let format = serializer.format();
        self.serializers.insert(format, serializer);
    }

    pub fn register_message_type<T: Message>(&mut self, preferred_format: Option<SerFormat>) {
        let type_name = std::any::type_name::<T>().to_string();
        let format = preferred_format.unwrap_or(self.strategy.local_preferred);

        self.type_mappings.insert(type_name.clone(), format);
        self.schema_versions.insert(type_name, T::SCHEMA_VERSION);
    }

    #[inline]
    pub fn get_serializer(&self, format: SerFormat) -> Result<&MessageSerializer> {
        self.serializers.get(&format).ok_or_else(|| {
            LyricoreActorError::Actor(crate::error::ActorError::RpcError(format!(
                "Serializer not found for format: {:?}",
                format
            )))
        })
    }

    #[inline]
    pub fn get_fast_serializer(&self, format: SerFormat) -> &MessageSerializer {
        match format {
            SerFormat::Json => &self.json_serializer,
            SerFormat::Messagepack => &self.messagepack_serializer,
            _ => self
                .serializers
                .get(&format)
                .unwrap_or(&self.json_serializer),
        }
    }

    pub fn choose_serializer(
        &self,
        message_type: &str,
        is_local: bool,
        remote_capabilities: Option<&actor_service::NodeCapabilities>,
    ) -> Result<&MessageSerializer> {
        let format = if is_local {
            self.type_mappings
                .get(message_type)
                .copied()
                .unwrap_or(self.strategy.local_preferred)
        } else if let Some(caps) = remote_capabilities {
            self.negotiate_format(message_type, caps)
        } else {
            self.strategy.cross_language_default
        };

        self.get_serializer(format)
    }

    fn negotiate_format(
        &self,
        message_type: &str,
        remote_capabilities: &actor_service::NodeCapabilities,
    ) -> SerFormat {
        let remote_formats: Vec<SerFormat> = remote_capabilities
            .supported_formats
            .iter()
            .filter_map(|&f| SerFormat::try_from(f).ok())
            .collect();

        if let Some(&preferred) = self.type_mappings.get(message_type) {
            if remote_formats.contains(&preferred) {
                return preferred;
            }
        }

        for &format in &self.strategy.remote_fallback {
            if remote_formats.contains(&format) {
                return format;
            }
        }

        self.strategy.cross_language_default
    }

    pub fn create_envelope<T: Message + Serialize>(
        &self,
        message: &T,
        is_local: bool,
        remote_capabilities: Option<&actor_service::NodeCapabilities>,
    ) -> Result<MessageEnvelope> {
        if is_local {
            self.create_local_envelope(message)
        } else {
            let message_type = MessageEnvelope::generate_message_type_id::<T>();
            let serializer =
                self.choose_serializer(&message_type, is_local, remote_capabilities)?;
            MessageEnvelope::new(message, serializer)
        }
    }

    #[inline]
    pub fn create_local_envelope<T: Message + Serialize>(
        &self,
        message: &T,
    ) -> Result<MessageEnvelope> {
        let serializer = self.get_fast_serializer(self.strategy.local_preferred);
        MessageEnvelope::new(message, serializer)
    }
}
