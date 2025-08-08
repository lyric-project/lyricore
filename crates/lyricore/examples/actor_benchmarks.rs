use futures::future::join_all;
use lyricore::serialization::{SerFormat, SerializationStrategy};
use lyricore::{ActorContext, Message, SchedulerConfig};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

struct FastActor {
    processed: AtomicU64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FastMessage {
    id: u64,
    data: String,
}

impl Message for FastMessage {
    type Response = String;
}

#[async_trait::async_trait]
impl lyricore::Actor for FastActor {
    type Message = FastMessage;
    type State = ();

    async fn on_start(&mut self, _ctx: &mut ActorContext) -> lyricore::error::Result<()> {
        Ok(())
    }

    async fn on_message(
        &mut self,
        _msg: Self::Message,
        _ctx: &mut ActorContext,
    ) -> lyricore::error::Result<()> {
        self.processed.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn handle_message(
        &mut self,
        msg: Self::Message,
        ctx: &mut ActorContext,
    ) -> lyricore::error::Result<String> {
        self.on_message(msg.clone(), ctx).await?;
        Ok(format!("Processed: {}", msg.id))
    }

    async fn on_stop(&mut self, _ctx: &mut ActorContext) -> lyricore::error::Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct TestResult {
    qps: f64,
    total_messages: usize,
    successful_messages: usize,
    duration: Duration,
    avg_latency_ms: f64,
    p95_latency_ms: f64,
    p99_latency_ms: f64,
}

impl TestResult {
    fn new(
        total_messages: usize,
        successful_messages: usize,
        duration: Duration,
        mut latencies: Vec<Duration>,
    ) -> Self {
        latencies.sort();

        let qps = successful_messages as f64 / duration.as_secs_f64();
        let avg_latency_ms = latencies
            .iter()
            .map(|d| d.as_secs_f64() * 1000.0)
            .sum::<f64>()
            / latencies.len() as f64;

        let p95_idx = (latencies.len() as f64 * 0.95) as usize;
        let p99_idx = (latencies.len() as f64 * 0.99) as usize;
        let p95_latency_ms = latencies[p95_idx.min(latencies.len() - 1)].as_secs_f64() * 1000.0;
        let p99_latency_ms = latencies[p99_idx.min(latencies.len() - 1)].as_secs_f64() * 1000.0;

        Self {
            qps,
            total_messages,
            successful_messages,
            duration,
            avg_latency_ms,
            p95_latency_ms,
            p99_latency_ms,
        }
    }

    fn print(&self, test_name: &str) {
        println!("✅ {} Results:", test_name);
        println!("   QPS: {:.0}", self.qps);
        println!(
            "   Success: {}/{}",
            self.successful_messages, self.total_messages
        );
        println!("   Duration: {:.2}s", self.duration.as_secs_f64());
        println!("   Avg Latency: {:.3}ms", self.avg_latency_ms);
        println!("   P95 Latency: {:.3}ms", self.p95_latency_ms);
        println!("   P99 Latency: {:.3}ms", self.p99_latency_ms);
    }
}

#[derive(Debug, Clone)]
enum SystemType {
    Optimized,
    Baseline,
}

#[derive(Debug, Clone)]
enum OperationType {
    Tell,
    Ask,
}

// Core testing function - all tests reuse this function
async fn run_performance_test(
    system_type: SystemType,
    operation_type: OperationType,
    actors: usize,
    messages_per_actor: usize,
    max_concurrent: usize,
    port: u16,
) -> lyricore::error::Result<TestResult> {
    let test_name = format!("{:?} {:?}", system_type, operation_type);
    println!("🚀 Starting {} test...", test_name);
    println!(
        "   Actors: {}, Messages/actor: {}, Concurrent: {}",
        actors, messages_per_actor, max_concurrent
    );

    let mut system = match system_type {
        SystemType::Optimized => lyricore::ActorSystem::new_optimized(
            format!("{}_node", test_name.replace(" ", "_").to_lowercase()),
            format!("127.0.0.1:{}", port),
            SchedulerConfig::default(),
            SerFormat::Json,
        )?,
        SystemType::Baseline => {
            let strategy = SerializationStrategy::fast_local();
            lyricore::ActorSystem::new(
                format!("{}_node", test_name.replace(" ", "_").to_lowercase()),
                format!("127.0.0.1:{}", port),
                SchedulerConfig::default(),
                Some(strategy),
            )?
        }
    };
    system.start_server().await?;

    // Create actors
    let actor_refs: Vec<_> = (0..actors)
        .map(|i| {
            system.spawn_local(
                format!("actor_{}_{}", test_name.replace(" ", "_").to_lowercase(), i),
                FastActor {
                    processed: AtomicU64::new(0),
                },
            )
        })
        .collect();

    let semaphore = Arc::new(Semaphore::new(max_concurrent));
    let global_latencies = Arc::new(std::sync::Mutex::new(Vec::new()));
    let global_stats = Arc::new(std::sync::Mutex::new((0usize, 0usize))); // (total, success)

    let start_time = Instant::now();

    // Send messages in batches per actor
    let msg_prefix = test_name.replace(" ", "_");
    let tasks: Vec<_> = actor_refs
        .iter()
        .enumerate()
        .map(|(actor_idx, actor_ref)| {
            let actor_ref = actor_ref.clone();
            let semaphore = semaphore.clone();
            let global_latencies = global_latencies.clone();
            let global_stats = global_stats.clone();
            let operation_type = operation_type.clone();
            let msg_prefix = msg_prefix.clone();

            async move {
                let _permit = semaphore.acquire().await.unwrap();
                let mut local_latencies = Vec::new();
                let mut local_success = 0;

                for msg_idx in 0..messages_per_actor {
                    let req_start = Instant::now();
                    let msg_id = (actor_idx * messages_per_actor + msg_idx) as u64;

                    let result = match operation_type {
                        OperationType::Tell => actor_ref
                            .tell(FastMessage {
                                id: msg_id,
                                data: format!("{}_{}", msg_prefix, msg_id),
                            })
                            .await
                            .is_ok(),
                        OperationType::Ask => actor_ref
                            .ask(FastMessage {
                                id: msg_id,
                                data: format!("{}_{}", msg_prefix, msg_id),
                            })
                            .await
                            .is_ok(),
                    };

                    let latency = req_start.elapsed();
                    local_latencies.push(latency);
                    if result {
                        local_success += 1;
                    }
                }

                {
                    let mut latencies = global_latencies.lock().unwrap();
                    latencies.extend(local_latencies);
                }
                {
                    let mut stats = global_stats.lock().unwrap();
                    stats.0 += messages_per_actor;
                    stats.1 += local_success;
                }

                local_success
            }
        })
        .collect();

    let _results = join_all(tasks).await;
    let total_duration = start_time.elapsed();

    let latencies = global_latencies.lock().unwrap().clone();
    let (total_messages, successful_messages) = *global_stats.lock().unwrap();

    let result = TestResult::new(
        total_messages,
        successful_messages,
        total_duration,
        latencies,
    );
    result.print(&test_name);

    Ok(result)
}

#[derive(Debug)]
struct ComparisonResult {
    test_name: String,
    baseline: TestResult,
    optimized: TestResult,
    qps_improvement: f64,
    latency_improvement: f64,
}

impl ComparisonResult {
    fn new(test_name: String, baseline: TestResult, optimized: TestResult) -> Self {
        let qps_improvement = ((optimized.qps - baseline.qps) / baseline.qps) * 100.0;
        let latency_improvement = ((baseline.avg_latency_ms - optimized.avg_latency_ms)
            / baseline.avg_latency_ms)
            * 100.0;

        Self {
            test_name,
            baseline,
            optimized,
            qps_improvement,
            latency_improvement,
        }
    }

    fn print(&self) {
        println!("\n📈 {} Comparison:", self.test_name);
        println!("   Baseline QPS:  {:.0}", self.baseline.qps);
        println!("   Optimized QPS: {:.0}", self.optimized.qps);
        if self.qps_improvement > 0.0 {
            println!("   🚀 QPS Improvement: +{:.1}%", self.qps_improvement);
        } else {
            println!("   📉 QPS Change: {:.1}%", self.qps_improvement);
        }

        println!("   Baseline Latency: {:.3}ms", self.baseline.avg_latency_ms);
        println!(
            "   Optimized Latency: {:.3}ms",
            self.optimized.avg_latency_ms
        );
        if self.latency_improvement > 0.0 {
            println!(
                "   ⚡ Latency Improvement: +{:.1}%",
                self.latency_improvement
            );
        } else {
            println!("   🐌 Latency Change: {:.1}%", self.latency_improvement);
        }
    }
}

async fn run_comparison_test(
    operation_type: OperationType,
    actors: usize,
    messages_per_actor: usize,
    max_concurrent: usize,
    base_port: u16,
) -> lyricore::error::Result<ComparisonResult> {
    let op_name = format!("{:?}", operation_type);

    let baseline = run_performance_test(
        SystemType::Baseline,
        operation_type.clone(),
        actors,
        messages_per_actor,
        max_concurrent,
        base_port,
    )
    .await?;

    tokio::time::sleep(Duration::from_secs(2)).await;

    let optimized = run_performance_test(
        SystemType::Optimized,
        operation_type,
        actors,
        messages_per_actor,
        max_concurrent,
        base_port + 1,
    )
    .await?;

    let comparison = ComparisonResult::new(op_name, baseline, optimized);
    comparison.print();

    Ok(comparison)
}

async fn comprehensive_test() -> lyricore::error::Result<Vec<ComparisonResult>> {
    println!("🎯 Comprehensive Performance Test");
    println!("=================================\n");

    let test_configs = [
        ("Small Scale", 100, 1000, 100),
        ("Medium Scale", 1000, 100, 200),
        ("Large Scale", 5000, 50, 500),
    ];

    let mut all_results = Vec::new();

    for (scale_name, actors, messages_per_actor, max_concurrent) in test_configs {
        println!(
            "🔥 {} Test: {} actors × {} msgs = {} total",
            scale_name,
            actors,
            messages_per_actor,
            actors * messages_per_actor
        );
        println!("================================================");

        // Tell
        let tell_comparison = run_comparison_test(
            OperationType::Tell,
            actors,
            messages_per_actor,
            max_concurrent,
            50200,
        )
        .await?;
        all_results.push(tell_comparison);

        tokio::time::sleep(Duration::from_secs(2)).await;

        // Ask
        let ask_comparison = run_comparison_test(
            OperationType::Ask,
            actors,
            messages_per_actor,
            max_concurrent,
            50210,
        )
        .await?;
        all_results.push(ask_comparison);

        println!("================================================\n");
        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    Ok(all_results)
}

async fn stress_test() -> lyricore::error::Result<(TestResult, TestResult)> {
    println!("💥 STRESS TEST - Maximum Performance");
    println!("====================================");

    let actors = 10000;
    let messages_per_actor = 100;
    let max_concurrent = 1000;

    println!(
        "🔥 Stress Config: {} actors × {} msgs = {} total",
        actors,
        messages_per_actor,
        actors * messages_per_actor
    );

    let tell_result = run_performance_test(
        SystemType::Optimized,
        OperationType::Tell,
        actors,
        messages_per_actor,
        max_concurrent,
        50220,
    )
    .await?;

    tokio::time::sleep(Duration::from_secs(3)).await;

    let ask_result = run_performance_test(
        SystemType::Optimized,
        OperationType::Ask,
        actors,
        messages_per_actor,
        max_concurrent,
        50230,
    )
    .await?;

    println!("\n🏆 MAXIMUM PERFORMANCE ACHIEVED:");
    println!("================================");
    println!("📤 TELL QPS: {:.0}", tell_result.qps);
    println!("📥 ASK QPS:  {:.0}", ask_result.qps);

    Ok((tell_result, ask_result))
}

fn print_final_summary(
    comparisons: &[ComparisonResult],
    stress_tell: &TestResult,
    stress_ask: &TestResult,
) {
    println!("\n{}", "=".repeat(60));
    println!("🏁 FINAL PERFORMANCE SUMMARY");
    println!("{}", "=".repeat(60));

    println!("\n📊 Performance Improvements:");
    println!("{}", "-".repeat(30));
    for comparison in comparisons {
        println!(
            "• {}: QPS +{:.1}%, Latency +{:.1}%",
            comparison.test_name, comparison.qps_improvement, comparison.latency_improvement
        );
    }

    println!("\n🚀 Peak Performance (Stress Test):");
    println!("{}", "-".repeat(35));
    println!(
        "• TELL: {:.0} QPS (avg: {:.3}ms, p99: {:.3}ms)",
        stress_tell.qps, stress_tell.avg_latency_ms, stress_tell.p99_latency_ms
    );
    println!(
        "• ASK:  {:.0} QPS (avg: {:.3}ms, p99: {:.3}ms)",
        stress_ask.qps, stress_ask.avg_latency_ms, stress_ask.p99_latency_ms
    );

    println!("\n🎯 Performance Goals Assessment:");
    println!("{}", "-".repeat(35));

    if stress_tell.qps >= 1_000_000.0 {
        println!("✅ TELL: 1M+ QPS target ACHIEVED!");
    } else if stress_tell.qps >= 500_000.0 {
        println!("🟡 TELL: 500K+ QPS achieved (target: 1M)");
    } else {
        println!("❌ TELL: Below 500K QPS");
    }

    if stress_ask.qps >= 100_000.0 {
        println!("✅ ASK: 100K+ QPS target ACHIEVED!");
    } else if stress_ask.qps >= 50_000.0 {
        println!("🟡 ASK: 50K+ QPS achieved (target: 100K)");
    } else {
        println!("❌ ASK: Below 50K QPS");
    }

    let avg_qps_improvement: f64 =
        comparisons.iter().map(|c| c.qps_improvement).sum::<f64>() / comparisons.len() as f64;

    let avg_latency_improvement: f64 = comparisons
        .iter()
        .map(|c| c.latency_improvement)
        .sum::<f64>()
        / comparisons.len() as f64;

    println!("\n📈 Overall Optimization Impact:");
    println!("{}", "-".repeat(35));
    println!("• Average QPS improvement: +{:.1}%", avg_qps_improvement);
    println!(
        "• Average latency improvement: +{:.1}%",
        avg_latency_improvement
    );

    if avg_qps_improvement > 20.0 {
        println!("🎉 EXCELLENT optimization results!");
    } else if avg_qps_improvement > 10.0 {
        println!("✅ GOOD optimization results!");
    } else if avg_qps_improvement > 0.0 {
        println!("🟡 Modest optimization gains");
    } else {
        println!("❌ Optimization needs improvement");
    }

    println!("{}", "=".repeat(60));
}

#[tokio::main]
async fn main() -> lyricore::error::Result<()> {
    println!("⚡ High Performance Actor System Benchmark");
    println!("==========================================\n");

    // Comprehensive performance tests
    let comparisons = comprehensive_test().await?;

    // Pressure test to find maximum QPS
    let (stress_tell, stress_ask) = stress_test().await?;

    // Final summary
    print_final_summary(&comparisons, &stress_tell, &stress_ask);

    Ok(())
}
