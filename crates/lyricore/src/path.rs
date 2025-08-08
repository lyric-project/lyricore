use crate::error::{ActorError, LyricoreActorError, Result};
use serde::{Deserialize, Serialize};
use std::hash::DefaultHasher;
use std::net::{SocketAddr, ToSocketAddrs};
use std::str::FromStr;

/// Actor network address
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ActorAddress {
    pub host: String,
    pub port: u16,
}

impl ActorAddress {
    pub fn new(host: String, port: u16) -> Self {
        Self { host, port }
    }

    pub fn local(port: u16) -> Self {
        Self {
            host: "localhost".to_string(),
            port,
        }
    }

    #[inline]
    pub fn is_local(&self) -> bool {
        self.host == "localhost" || self.host == "127.0.0.1" || self.host == "::1"
    }

    /// Check if the address can be resolved to a valid SocketAddr
    pub fn validate(&self) -> Result<()> {
        let addr_str = format!("{}:{}", self.host, self.port);

        // Try to resolve the address to ensure it's valid
        match addr_str.to_socket_addrs() {
            Ok(mut addrs) => {
                if addrs.next().is_some() {
                    Ok(())
                } else {
                    Err(LyricoreActorError::Actor(ActorError::InvalidState(
                        format!("Cannot resolve address: {}", addr_str),
                    )))
                }
            }
            Err(e) => Err(LyricoreActorError::Actor(ActorError::InvalidState(
                format!("Invalid address {}: {}", addr_str, e),
            ))),
        }
    }

    /// Get the string representation of the address
    pub fn to_socket_addr(&self) -> Result<SocketAddr> {
        let addr_str = format!("{}:{}", self.host, self.port);
        addr_str
            .to_socket_addrs()
            .map_err(|e| {
                LyricoreActorError::Actor(ActorError::InvalidState(format!(
                    "Cannot resolve address {}: {}",
                    addr_str, e
                )))
            })?
            .next()
            .ok_or_else(|| {
                LyricoreActorError::Actor(ActorError::InvalidState(format!(
                    "No valid address found for: {}",
                    addr_str
                )))
            })
    }
}

impl std::fmt::Display for ActorAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

impl FromStr for ActorAddress {
    type Err = LyricoreActorError;

    fn from_str(s: &str) -> Result<Self> {
        // Handle IPv6 addresses in brackets, e.g. [::1]:8080
        if s.starts_with('[') {
            if let Some(bracket_end) = s.find(']') {
                let host = s[1..bracket_end].to_string();
                let remainder = &s[bracket_end + 1..];

                if !remainder.starts_with(':') {
                    return Err(LyricoreActorError::Actor(ActorError::InvalidState(
                        format!("Invalid IPv6 address format: {}", s),
                    )));
                }

                let port = remainder[1..].parse().map_err(|_| {
                    LyricoreActorError::Actor(ActorError::InvalidState(format!(
                        "Invalid port in address: {}",
                        s
                    )))
                })?;

                let addr = Self { host, port };
                // Validate the address format
                addr.validate()?;
                return Ok(addr);
            }
        }

        // Handle the case where the address is in the format host:port
        let parts: Vec<&str> = s.rsplitn(2, ':').collect();
        if parts.len() != 2 {
            return Err(LyricoreActorError::Actor(ActorError::InvalidState(
                format!("Invalid address format: {} (expected host:port)", s),
            )));
        }

        let port_str = parts[0];
        let host = parts[1].to_string();

        let port = port_str.parse().map_err(|_| {
            LyricoreActorError::Actor(ActorError::InvalidState(format!(
                "Invalid port '{}' in address: {}",
                port_str, s
            )))
        })?;

        let addr = Self { host, port };

        // Validate the address format(optional, if you want to validate immediately)
        // addr.validate()?;
        Ok(addr)
    }
}

/// Actor path
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ActorPath {
    pub protocol: String,      // "lyricore"
    pub system: String,        // "test_system"
    pub address: ActorAddress, // network address, e.g. "localhost:8888"
    pub path: String,          // "/user/my_actor"
}

impl ActorPath {
    pub fn new(system: String, address: ActorAddress, path: String) -> Self {
        Self {
            protocol: "lyricore".to_string(),
            system,
            address,
            path: if path.starts_with('/') {
                path
            } else {
                format!("/{}", path)
            },
        }
    }

    /// Create local ActorPath
    pub fn local(system: String, port: u16, path: String) -> Self {
        Self::new(system, ActorAddress::local(port), path)
    }

    /// full: lyricore://system@host:port/path
    pub fn full_path(&self) -> String {
        format!(
            "{}://{}@{}{}",
            self.protocol, self.system, self.address, self.path
        )
    }

    /// local path: lyricore://system/path (no address)
    pub fn local_path(&self) -> String {
        format!("{}://{}{}", self.protocol, self.system, self.path)
    }

    /// System address(system@host:port) e.g. "test_system@localhost:8888"
    pub fn system_address(&self) -> String {
        format!("{}@{}", self.system, self.address)
    }

    /// Whether this path is local to the current system and address
    pub fn is_local(&self, current_system: &str, current_address: &ActorAddress) -> bool {
        self.system == current_system && self.address == *current_address
    }

    /// Get the full system address (protocol + system + address)
    pub fn parent(&self) -> Option<ActorPath> {
        if self.path == "/" {
            return None;
        }

        let parent_path = if let Some(last_slash) = self.path.rfind('/') {
            if last_slash == 0 {
                "/".to_string()
            } else {
                self.path[..last_slash].to_string()
            }
        } else {
            return None;
        };

        Some(ActorPath {
            protocol: self.protocol.clone(),
            system: self.system.clone(),
            address: self.address.clone(),
            path: parent_path,
        })
    }

    /// Create a child ActorPath with the given name
    pub fn child(&self, name: &str) -> ActorPath {
        let child_path = if self.path.ends_with('/') {
            format!("{}{}", self.path, name)
        } else {
            format!("{}/{}", self.path, name)
        };

        ActorPath {
            protocol: self.protocol.clone(),
            system: self.system.clone(),
            address: self.address.clone(),
            path: child_path,
        }
    }

    /// Get the name of the actor (the last part of the path)
    pub fn name(&self) -> &str {
        if let Some(last_slash) = self.path.rfind('/') {
            &self.path[last_slash + 1..]
        } else {
            &self.path
        }
    }

    /// Parse an ActorPath from a string
    pub fn parse(path_str: &str) -> Result<Self> {
        // Two formats are supported:
        // 1. lyricore://system@host:port/path
        // 2. lyricore://system/path (local path, no address)

        if !path_str.starts_with("lyricore://") {
            return Err(LyricoreActorError::Actor(ActorError::InvalidState(
                format!("Invalid protocol in path: {}", path_str),
            )));
        }

        let without_protocol = &path_str[11..]; // remove "lyricore://"

        // Find the start of the path
        let path_start = without_protocol.find('/').unwrap_or(without_protocol.len());
        let system_part = &without_protocol[..path_start];
        let path = if path_start < without_protocol.len() {
            without_protocol[path_start..].to_string()
        } else {
            "/".to_string()
        };

        // Parse system part
        if let Some(at_pos) = system_part.find('@') {
            // Full: system@host:port
            let system = system_part[..at_pos].to_string();
            let address_str = &system_part[at_pos + 1..];
            let address = ActorAddress::from_str(address_str)?;

            Ok(ActorPath {
                protocol: "lyricore".to_string(),
                system,
                address,
                path,
            })
        } else {
            Ok(ActorPath {
                protocol: "lyricore".to_string(),
                system: system_part.to_string(),
                address: ActorAddress::local(0), // Default to 0
                path,
            })
        }
    }

    /// Parse from a simplified path format, using a default address if no address is provided.
    pub fn parse_with_default(path_str: &str, default_address: &ActorAddress) -> Result<Self> {
        if !path_str.starts_with("lyricore://") {
            return Err(LyricoreActorError::Actor(ActorError::InvalidState(
                format!("Invalid protocol in path: {}", path_str),
            )));
        }

        let without_protocol = &path_str[11..];
        let path_start = without_protocol.find('/').unwrap_or(without_protocol.len());
        let system_part = &without_protocol[..path_start];
        let path = if path_start < without_protocol.len() {
            without_protocol[path_start..].to_string()
        } else {
            "/".to_string()
        };

        if system_part.contains('@') {
            // Has address information, use full parsing
            Self::parse(path_str)
        } else {
            // Simple format: lyricore://system/path
            // Use default address
            Ok(ActorPath {
                protocol: "lyricore".to_string(),
                system: system_part.to_string(),
                address: default_address.clone(),
                path,
            })
        }
    }
}

impl std::fmt::Display for ActorPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.full_path())
    }
}

impl TryFrom<&str> for ActorPath {
    type Error = LyricoreActorError;

    fn try_from(value: &str) -> Result<Self> {
        Self::parse(value)
    }
}

impl TryFrom<String> for ActorPath {
    type Error = LyricoreActorError;

    fn try_from(value: String) -> Result<Self> {
        Self::parse(&value)
    }
}

/// Actor ID - Includes path information and internal runtime identifier
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorId {
    pub runtime_id: String, // UUID，for internal use scheduler and runtime identification
    pub path: ActorPath,    // Full actor pat
}

/// Just compare the path for equality
impl PartialEq for ActorId {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}

impl Eq for ActorId {}

/// Implement Hash for ActorId, just hash the path
impl std::hash::Hash for ActorId {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.path.hash(state);
    }
}

impl ActorId {
    pub fn new(runtime_id: String, path: ActorPath) -> Self {
        Self { runtime_id, path }
    }

    pub fn generate(path: ActorPath) -> Self {
        use uuid::Uuid;
        Self {
            runtime_id: Uuid::new_v4().to_string(),
            path,
        }
    }

    /// Get the node identifier (for compatibility with existing code)
    pub fn node_id(&self) -> String {
        self.path.system_address()
    }

    /// Returns the runtime ID, which is a UUID for internal use.
    pub fn value(&self) -> &str {
        &self.runtime_id
    }

    pub fn runtime_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        self.runtime_id.hash(&mut hasher);
        self.path.hash(&mut hasher);
        hasher.finish()
    }
}

impl std::fmt::Display for ActorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.runtime_id.is_empty() {
            write!(f, "{}", self.path.full_path())
        } else {
            write!(f, "{}#{}", self.path, self.runtime_id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_actor_address() {
        let addr = ActorAddress::new("localhost".to_string(), 8888);
        assert_eq!(addr.to_string(), "localhost:8888");
        assert!(addr.is_local());

        let addr2: ActorAddress = "192.168.1.100:9090".parse().unwrap();
        assert_eq!(addr2.host, "192.168.1.100");
        assert_eq!(addr2.port, 9090);
        assert!(!addr2.is_local());

        // Test domain address parsing
        let domain_addr: ActorAddress = "example.com:80".parse().unwrap();
        assert_eq!(domain_addr.host, "example.com");
        assert_eq!(domain_addr.port, 80);

        // Test IPv6 address parsing
        let ipv6_addr: ActorAddress = "[::1]:8080".parse().unwrap();
        assert_eq!(ipv6_addr.host, "::1");
        assert_eq!(ipv6_addr.port, 8080);
        assert!(ipv6_addr.is_local());
    }

    #[test]
    fn test_address_validation() {
        let addr = ActorAddress::new("localhost".to_string(), 8888);
        assert!(addr.validate().is_ok());

        let domain_addr = ActorAddress::new("example.com".to_string(), 80);
        // domain_addr.validate() May fail if DNS resolution is not available
    }

    #[test]
    fn test_actor_path() {
        let addr = ActorAddress::new("localhost".to_string(), 8888);
        let path = ActorPath::new(
            "test_system".to_string(),
            addr,
            "/user/my_actor".to_string(),
        );

        assert_eq!(
            path.full_path(),
            "lyricore://test_system@localhost:8888/user/my_actor"
        );
        assert_eq!(path.local_path(), "lyricore://test_system/user/my_actor");
        assert_eq!(path.name(), "my_actor");

        let child = path.child("child1");
        assert_eq!(child.path, "/user/my_actor/child1");

        let parent = child.parent().unwrap();
        assert_eq!(parent.path, "/user/my_actor");
    }

    #[test]
    fn test_path_parsing() {
        let path_str = "lyricore://test_system@localhost:8888/user/my_actor";
        let path = ActorPath::parse(path_str).unwrap();

        assert_eq!(path.system, "test_system");
        assert_eq!(path.address.host, "localhost");
        assert_eq!(path.address.port, 8888);
        assert_eq!(path.path, "/user/my_actor");

        // Test domain address parsing
        let domain_path_str = "lyricore://test_system@example.com:80/user/my_actor";
        let domain_path = ActorPath::parse(domain_path_str).unwrap();
        assert_eq!(domain_path.address.host, "example.com");
        assert_eq!(domain_path.address.port, 80);

        let default_addr = ActorAddress::local(8888);
        let simple_path = "lyricore://test_system/user/my_actor";
        let parsed = ActorPath::parse_with_default(simple_path, &default_addr).unwrap();
        assert_eq!(parsed.system, "test_system");
        assert_eq!(parsed.address, default_addr);
    }
}
