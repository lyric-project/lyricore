use crate::actor_ref::{ActorRef, LocalActorRef};
use crate::actor_system::ActorSystemInner;
use crate::error::Result;
use crate::path::{ActorId, ActorPath};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::any::TypeId;
use std::sync::Arc;

pub trait Message: Send + Sync + 'static + Clone + Serialize + for<'de> Deserialize<'de> {
    type Response: Send + Sync + 'static + Default + Serialize + for<'de> Deserialize<'de> + Message;
    const SCHEMA_VERSION: u32 = 1;

    fn type_id() -> TypeId {
        TypeId::of::<Self>()
    }

    fn type_name() -> &'static str {
        std::any::type_name::<Self>()
    }
}

impl<T> Message for T
where
    T: Send + Sync + 'static + Clone + Serialize + for<'de> Deserialize<'de> + Default,
{
    type Response = ();
    const SCHEMA_VERSION: u32 = 1;
}

#[derive(Clone, Debug)]
pub struct ActorContext {
    pub actor_id: ActorId,
    // Self reference to the actor, send messages to itself
    pub actor_ref: LocalActorRef,
    // Actor system reference, used for system-level operations
    system_ref: Arc<ActorSystemInner>,
    pub(crate) sender: Option<ActorRef>,
}

impl ActorContext {
    pub(crate) fn new(
        actor_id: ActorId,
        actor_ref: LocalActorRef,
        system_ref: Arc<ActorSystemInner>,
        sender: Option<ActorRef>,
    ) -> Self {
        Self {
            actor_id,
            actor_ref,
            system_ref,
            sender,
        }
    }

    pub fn actor_id(&self) -> &ActorId {
        &self.actor_id
    }

    pub fn self_ref(&self) -> &LocalActorRef {
        &self.actor_ref
    }

    pub async fn tell_self<M: Message + Serialize + 'static>(&self, message: M) -> Result<()> {
        self.actor_ref.tell(message).await
    }
    pub async fn actor_of_path(&self, path: &ActorPath) -> Result<ActorRef> {
        self.system_ref.actor_of_path(path).await
    }

    pub async fn actor_of_id(&self, actor_id: ActorId) -> Result<ActorRef> {
        self.system_ref.actor_of_path(&actor_id.path).await
    }

    pub async fn actor_of(&self, path_str: &str) -> Result<ActorRef> {
        let path = if path_str.starts_with("lyricore://") {
            ActorPath::parse_with_default(path_str, &self.system_ref.system_address)?
        } else {
            // Relative paths are resolved based on the current actor's path
            if path_str.starts_with('/') {
                // 绝对路径
                ActorPath::new(
                    self.system_ref.system_name.clone(),
                    self.system_ref.system_address.clone(),
                    path_str.to_string(),
                )
            } else {
                // Relative paths are resolved based on the current actor's path
                if let Some(parent) = self.actor_id.path.parent() {
                    parent.child(path_str)
                } else {
                    ActorPath::new(
                        self.system_ref.system_name.clone(),
                        self.system_ref.system_address.clone(),
                        format!("/{}", path_str),
                    )
                }
            }
        };
        self.system_ref.actor_of_path(&path).await
    }

    /// Creates a child actor with the given name.
    pub fn spawn<A: Actor + 'static>(&self, name: &str, actor: A) -> Result<ActorRef> {
        let child_path = self.actor_id.path.child(name);
        self.system_ref.spawn_at_path(child_path, actor)
    }

    // Spawn an actor at a specific path
    pub fn spawn_at<A: Actor + 'static>(&self, path: &str, actor: A) -> Result<ActorRef> {
        let actor_path = if path.starts_with('/') {
            ActorPath::new(
                self.system_ref.system_name.clone(),
                self.system_ref.system_address.clone(),
                path.to_string(),
            )
        } else {
            self.actor_id.path.child(path)
        };
        self.system_ref.spawn_at_path(actor_path, actor)
    }
}

#[async_trait]
pub trait Actor: Send + Sync + 'static {
    type Message: Message + Send + 'static;
    type State: Send + Sync + 'static;

    /// Invoked when the actor is started
    async fn on_start(&mut self, _ctx: &mut ActorContext) -> Result<()> {
        Ok(())
    }

    /// Handle `tell` messages(fire-and-forget)
    async fn on_message(&mut self, msg: Self::Message, ctx: &mut ActorContext) -> Result<()>;

    /// Handle messages that expect a response (ask pattern)
    async fn handle_message(
        &mut self,
        msg: Self::Message,
        ctx: &mut ActorContext,
    ) -> Result<<Self::Message as Message>::Response> {
        self.on_message(msg, ctx).await?;
        Ok(<Self::Message as Message>::Response::default())
    }

    /// Invoked when the actor is stopped
    async fn on_stop(&mut self, ctx: &mut ActorContext) -> Result<()> {
        Ok(())
    }
}
