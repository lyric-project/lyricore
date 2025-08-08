use lyricore::{Actor, ActorContext, ActorSystem, Message, SchedulerConfig};
use serde::{Deserialize, Serialize};
use std::time::Duration;

struct SuperActor {
    name: String,
    cnt: u32,
}

struct ChildActor {
    name: String,
    cnt: u32,
}

#[async_trait::async_trait]
impl Actor for ChildActor {
    type Message = String;
    type State = ();

    async fn on_message(
        &mut self,
        msg: Self::Message,
        ctx: &mut ActorContext,
    ) -> lyricore::Result<()> {
        self.cnt += 1;
        println!(
            "Child Actor {} received message: {}, count: {}",
            self.name, msg, self.cnt
        );
        Ok(())
    }
}

#[async_trait::async_trait]
impl Actor for SuperActor {
    type Message = String;
    type State = ();

    async fn on_message(
        &mut self,
        msg: Self::Message,
        ctx: &mut ActorContext,
    ) -> lyricore::Result<()> {
        for i in 0..55 {
            let child_name = format!("child_{}_{}", self.name, i);
            let child_actor = ChildActor {
                name: child_name.clone(),
                cnt: 0,
            };
            let child_ref = ctx.spawn(&child_name, child_actor)?;
            // Can't use `ask` here because current actor is not completed yet, it may block the
            // scheduler shared queue
            let _ = child_ref.tell(format!("Hello from {}", self.name)).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyricore::ActorSystem;

    #[tokio::test]
    async fn test_sub_actor() {
        let mut system = ActorSystem::new(
            "precise_test_node".to_string(),
            "127.0.0.1:50070".to_string(),
            SchedulerConfig::default(),
            None,
        )
        .unwrap();

        system.start_server().await.unwrap();
        let sup_ref = system.spawn_local(
            "super_actor".to_string(),
            SuperActor {
                name: "super_actor".to_string(),
                cnt: 0,
            },
        );
        let res = sup_ref.ask("Hello, World!".to_string()).await.unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await; // Wait for child actors to process messages
    }
}
