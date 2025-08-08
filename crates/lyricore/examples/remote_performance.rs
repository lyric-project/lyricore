use futures::stream::{FuturesUnordered, StreamExt};
use lyricore::{error, ActorContext, ActorPath, ActorSystem, Message, SchedulerConfig};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

struct TestActor {
    count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TestMessage {
    content: String,
}

impl Message for TestMessage {
    type Response = String;
}

#[async_trait::async_trait]
impl lyricore::Actor for TestActor {
    type Message = TestMessage;
    type State = ();

    async fn on_start(&mut self, _ctx: &mut ActorContext) -> lyricore::error::Result<()> {
        Ok(())
    }

    async fn on_message(
        &mut self,
        _msg: Self::Message,
        _ctx: &mut ActorContext,
    ) -> error::Result<()> {
        self.count += 1;
        Ok(())
    }

    async fn handle_message(
        &mut self,
        msg: Self::Message,
        ctx: &mut ActorContext,
    ) -> lyricore::error::Result<String> {
        self.on_message(msg.clone(), ctx).await?;
        Ok(format!("Response to: {}", msg.content))
    }

    async fn on_stop(&mut self, ctx: &mut ActorContext) -> lyricore::error::Result<()> {
        println!(
            "TestActor stopped: {} (processed {} messages)",
            ctx.actor_id, self.count
        );
        Ok(())
    }
}

#[derive(Clone)]
struct PerformanceStats {
    total_messages: usize,
    total_duration: Duration,
    qps: f64,
    avg_latency_ms: f64,
    p95_latency_ms: f64,
    p99_latency_ms: f64,
    cpu_efficiency: f64,
}

impl PerformanceStats {
    fn new(total_messages: usize, total_duration: Duration, latencies: &mut [Duration]) -> Self {
        latencies.sort();

        let qps = total_messages as f64 / total_duration.as_secs_f64();
        let avg_latency_ms = latencies
            .iter()
            .map(|d| d.as_secs_f64() * 1000.0)
            .sum::<f64>()
            / latencies.len() as f64;

        let p95_index = (latencies.len() as f64 * 0.95) as usize;
        let p99_index = (latencies.len() as f64 * 0.99) as usize;
        let p95_latency_ms = latencies[p95_index.min(latencies.len() - 1)].as_secs_f64() * 1000.0;
        let p99_latency_ms = latencies[p99_index.min(latencies.len() - 1)].as_secs_f64() * 1000.0;

        // CPU效率：QPS per core (假设使用8核)
        let cpu_efficiency = qps / 8.0;

        Self {
            total_messages,
            total_duration,
            qps,
            avg_latency_ms,
            p95_latency_ms,
            p99_latency_ms,
            cpu_efficiency,
        }
    }

    fn print_analysis(&self, test_name: &str) {
        println!("=== {} Analysis ===", test_name);
        println!("🚀 QPS: {:.2}", self.qps);
        println!("⚡ Avg Latency: {:.3}ms", self.avg_latency_ms);
        println!("📊 P95 Latency: {:.3}ms", self.p95_latency_ms);
        println!("🎯 CPU Efficiency: {:.2} QPS/core", self.cpu_efficiency);
        println!("⏱️  Duration: {:.2}s", self.total_duration.as_secs_f64());

        // 性能等级评估
        if self.qps > 50000.0 {
            println!("🏆 Performance Grade: EXCELLENT");
        } else if self.qps > 30000.0 {
            println!("🥈 Performance Grade: GOOD");
        } else if self.qps > 20000.0 {
            println!("🥉 Performance Grade: FAIR");
        } else {
            println!("📉 Performance Grade: NEEDS IMPROVEMENT");
        }
        println!("================================\n");
    }

    fn compare_with(&self, other: &PerformanceStats, other_name: &str) {
        let qps_improvement = (self.qps / other.qps - 1.0) * 100.0;
        let latency_improvement = (other.avg_latency_ms / self.avg_latency_ms - 1.0) * 100.0;

        println!("=== Performance Comparison ===");
        if qps_improvement > 0.0 {
            println!(
                "🚀 QPS Improvement: +{:.1}% vs {}",
                qps_improvement, other_name
            );
        } else {
            println!("📉 QPS Change: {:.1}% vs {}", qps_improvement, other_name);
        }

        if latency_improvement > 0.0 {
            println!(
                "⚡ Latency Improvement: +{:.1}% vs {}",
                latency_improvement, other_name
            );
        } else {
            println!(
                "🐌 Latency Change: {:.1}% vs {}",
                latency_improvement, other_name
            );
        }
        println!("==============================\n");
    }
}

async fn setup_cross_node_system() -> lyricore::error::Result<(ActorSystem, ActorSystem)> {
    let mut system1 = ActorSystem::new(
        "node1".to_string(),
        "127.0.0.1:50051".to_string(),
        SchedulerConfig::default(),
        None,
    )?;

    let mut system2 = ActorSystem::new(
        "node2".to_string(),
        "127.0.0.1:50052".to_string(),
        SchedulerConfig::default(),
        None,
    )?;

    system1.start_server().await?;
    system2.start_server().await?;
    tokio::time::sleep(Duration::from_millis(200)).await;

    system1
        .connect_to_node("node2".to_string(), "127.0.0.1:50052".to_string())
        .await?;
    system2
        .connect_to_node("node1".to_string(), "127.0.0.1:50051".to_string())
        .await?;
    tokio::time::sleep(Duration::from_millis(100)).await;

    Ok((system1, system2))
}

// Cross-node optimal streaming (directly comparable to single-node version)
async fn run_cross_node_optimal_streaming(
    actor_count: usize,
    messages_per_actor: usize,
    concurrency_limit: usize,
    wait_reply: bool,
) -> lyricore::error::Result<PerformanceStats> {
    println!("🏆 Running CROSS-NODE OPTIMAL STREAMING test...");

    let (system1, system2) = setup_cross_node_system().await?;

    // Create actors on node1
    let mut actors = Vec::with_capacity(actor_count);
    for i in 0..actor_count {
        system1.spawn_at(&format!("actor_{}", i), TestActor { count: 0 });

        // Get remote actor reference from node2
        let remote_path =
            ActorPath::try_from(format!("lyricore://node1@127.0.0.1:50051/user/actor_{}", i))
                .unwrap();

        if let Ok(remote_actor) = system2.actor_of_path(&remote_path).await {
            actors.push(remote_actor);
        }
    }

    // Make sure all remote actor references are obtained
    if actors.len() != actor_count {
        return Err(error::LyricoreActorError::Actor(
            error::ActorError::ActorNotFound("Failed to get all remote actors".to_string()),
        ));
    }

    let test_start = Instant::now();
    let mut latencies = Vec::new();
    let mut total_successful = 0;
    let mut futures = FuturesUnordered::new();

    // Create tasks in the simplest loop (identical to single-node version)
    for actor_idx in 0..actor_count {
        for msg_idx in 0..messages_per_actor {
            let actor_ref = actors[actor_idx].clone();

            let task = async move {
                let req_start = Instant::now();
                let result = if wait_reply {
                    actor_ref
                        .ask(TestMessage {
                            content: format!("msg_{}", msg_idx),
                        })
                        .await
                        .is_ok()
                } else {
                    actor_ref
                        .tell(TestMessage {
                            content: format!("msg_{}", msg_idx),
                        })
                        .await
                        .is_ok()
                };
                (req_start.elapsed(), result)
            };

            futures.push(task);

            // Simple backpressure control (identical to single-node version)
            if futures.len() >= concurrency_limit {
                if let Some((latency, is_success)) = futures.next().await {
                    latencies.push(latency);
                    if is_success {
                        total_successful += 1;
                    }
                }
            }
        }
    }

    // Handle remaining futures (identical to single-node version)
    while let Some((latency, is_success)) = futures.next().await {
        latencies.push(latency);
        if is_success {
            total_successful += 1;
        }
    }

    let total_duration = test_start.elapsed();
    let stats = PerformanceStats::new(total_successful, total_duration, &mut latencies);
    stats.print_analysis("CROSS-NODE OPTIMAL STREAMING");

    Ok(stats)
}

// Simple and direct actor creation
async fn run_single_node_optimal_streaming(
    actor_count: usize,
    messages_per_actor: usize,
    concurrency_limit: usize,
    wait_reply: bool,
) -> lyricore::error::Result<PerformanceStats> {
    println!("🏆 Running SINGLE-NODE OPTIMAL STREAMING test...");

    let mut system = lyricore::ActorSystem::new(
        "single_node".to_string(),
        "127.0.0.1:50070".to_string(),
        SchedulerConfig::default(),
        None,
    )?;

    system.start_server().await?;

    // Single-node actor creation
    let mut actors = Vec::with_capacity(actor_count);
    for i in 0..actor_count {
        actors.push(system.spawn_local(format!("actor_{}", i), TestActor { count: 0 }));
    }

    let test_start = Instant::now();
    let mut latencies = Vec::new();
    let mut total_successful = 0;
    let mut futures = FuturesUnordered::new();

    for actor_idx in 0..actor_count {
        for msg_idx in 0..messages_per_actor {
            let actor_ref = actors[actor_idx].clone();

            let task = async move {
                let req_start = Instant::now();
                let result = if wait_reply {
                    actor_ref
                        .ask(TestMessage {
                            content: format!("msg_{}", msg_idx),
                        })
                        .await
                        .is_ok()
                } else {
                    actor_ref
                        .tell(TestMessage {
                            content: format!("msg_{}", msg_idx),
                        })
                        .await
                        .is_ok()
                };
                (req_start.elapsed(), result)
            };

            futures.push(task);

            // Simple backpressure control
            if futures.len() >= concurrency_limit {
                if let Some((latency, is_success)) = futures.next().await {
                    latencies.push(latency);
                    if is_success {
                        total_successful += 1;
                    }
                }
            }
        }
    }

    // Handle remaining futures
    while let Some((latency, is_success)) = futures.next().await {
        latencies.push(latency);
        if is_success {
            total_successful += 1;
        }
    }

    let total_duration = test_start.elapsed();
    let stats = PerformanceStats::new(total_successful, total_duration, &mut latencies);
    stats.print_analysis("SINGLE-NODE OPTIMAL STREAMING");

    Ok(stats)
}

// Cross-node high concurrency test (different concurrency control)
async fn run_cross_node_high_concurrency(
    actor_count: usize,
    messages_per_actor: usize,
    high_concurrency_limit: usize,
    wait_reply: bool,
) -> lyricore::error::Result<PerformanceStats> {
    println!("🚀 Running CROSS-NODE HIGH CONCURRENCY test...");

    let (system1, system2) = setup_cross_node_system().await?;

    // Create actors on node1
    let mut actors = Vec::with_capacity(actor_count);
    for i in 0..actor_count {
        system1.spawn_at(&format!("high_conc_actor_{}", i), TestActor { count: 0 });

        let remote_path = ActorPath::try_from(format!(
            "lyricore://node1@127.0.0.1:50051/user/high_conc_actor_{}",
            i
        ))
        .unwrap();

        if let Ok(remote_actor) = system2.actor_of_path(&remote_path).await {
            actors.push(remote_actor);
        }
    }

    if actors.len() != actor_count {
        return Err(error::LyricoreActorError::Actor(
            error::ActorError::ActorNotFound("Failed to get all remote actors".to_string()),
        ));
    }

    let test_start = Instant::now();
    let mut latencies = Vec::new();
    let mut total_successful = 0;
    let mut futures = FuturesUnordered::new();

    for actor_idx in 0..actor_count {
        for msg_idx in 0..messages_per_actor {
            let actor_ref = actors[actor_idx].clone();

            let task = async move {
                let req_start = Instant::now();
                let result = if wait_reply {
                    actor_ref
                        .ask(TestMessage {
                            content: format!("msg_{}", msg_idx),
                        })
                        .await
                        .is_ok()
                } else {
                    actor_ref
                        .tell(TestMessage {
                            content: format!("msg_{}", msg_idx),
                        })
                        .await
                        .is_ok()
                };
                (req_start.elapsed(), result)
            };

            futures.push(task);

            if futures.len() >= high_concurrency_limit {
                if let Some((latency, is_success)) = futures.next().await {
                    latencies.push(latency);
                    if is_success {
                        total_successful += 1;
                    }
                }
            }
        }
    }

    while let Some((latency, is_success)) = futures.next().await {
        latencies.push(latency);
        if is_success {
            total_successful += 1;
        }
    }

    let total_duration = test_start.elapsed();
    let stats = PerformanceStats::new(total_successful, total_duration, &mut latencies);
    stats.print_analysis("CROSS-NODE HIGH CONCURRENCY");

    Ok(stats)
}

#[tokio::main]
async fn main() -> lyricore::error::Result<()> {
    println!("🎯 Unified Optimal Streaming: Single-Node vs Cross-Node\n");
    println!("Using identical code patterns for fair comparison:");
    println!("1. 🏠 Single-Node Optimal Streaming");
    println!("2. 🌐 Cross-Node Optimal Streaming (same algorithm)");
    println!("3. 🚀 Cross-Node High Concurrency");
    println!();

    // 4w QPS
    // let actor_count = 50;
    // let messages_per_actor = 20000; // 100K total messages

    let actor_count = 50;
    let messages_per_actor = 20000; // 100K total messages
    let concurrency_limit = 1000;
    let high_concurrency_limit = 2000;
    let wait_reply = true;

    println!(
        "Test Configuration: {} actors × {} msgs = {} total\n",
        actor_count,
        messages_per_actor,
        actor_count * messages_per_actor
    );

    // 1. Single-node optimal streaming
    let single_node_stats = run_single_node_optimal_streaming(
        actor_count,
        messages_per_actor,
        concurrency_limit,
        wait_reply,
    )
    .await?;

    tokio::time::sleep(Duration::from_millis(200)).await;

    // 2. Cross-node optimal streaming
    let cross_node_stats = run_cross_node_optimal_streaming(
        actor_count,
        messages_per_actor,
        concurrency_limit,
        wait_reply,
    )
    .await?;
    cross_node_stats.compare_with(&single_node_stats, "SINGLE-NODE");

    tokio::time::sleep(Duration::from_millis(200)).await;

    // let cross_node_stats = single_node_stats.clone(); // Placeholder to allow compilation
    // 3. Cross-node high concurrency test
    let high_conc_stats = run_cross_node_high_concurrency(
        actor_count,
        messages_per_actor,
        high_concurrency_limit,
        wait_reply,
    )
    .await?;
    high_conc_stats.compare_with(&cross_node_stats, "CROSS-NODE OPTIMAL");

    println!("🏆 Final Performance Summary:");
    println!(
        "Single-Node:       {:.2} QPS, {:.2}ms latency",
        single_node_stats.qps, single_node_stats.avg_latency_ms
    );
    println!(
        "Cross-Node:        {:.2} QPS, {:.2}ms latency ({:.1}x overhead)",
        cross_node_stats.qps,
        cross_node_stats.avg_latency_ms,
        single_node_stats.qps / cross_node_stats.qps
    );
    println!(
        "High Concurrency:  {:.2} QPS, {:.2}ms latency",
        high_conc_stats.qps, high_conc_stats.avg_latency_ms
    );

    println!("\n💡 Key Insights:");
    println!("• Cross-node adds network overhead but maintains algorithm efficiency");
    println!("• Identical code patterns ensure fair performance comparison");
    println!("• Network latency is the primary performance differentiator");
    println!("• Higher concurrency can compensate for network overhead");

    // Compute network overhead factor
    let network_overhead_factor = single_node_stats.qps / cross_node_stats.qps;
    println!("\n🌐 Network Analysis:");
    println!("• Network overhead factor: {:.2}x", network_overhead_factor);
    println!(
        "• Cross-node latency increase: {:.1}x",
        cross_node_stats.avg_latency_ms / single_node_stats.avg_latency_ms
    );

    if network_overhead_factor < 2.0 {
        println!("✅ Excellent cross-node performance!");
    } else if network_overhead_factor < 5.0 {
        println!("📊 Good cross-node performance");
    } else {
        println!("⚠️  Significant network overhead detected");
    }

    Ok(())
}
