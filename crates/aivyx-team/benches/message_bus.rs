use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};

use aivyx_team::message_bus::{MessageBus, TeamMessage};

fn make_message(from: &str, to: &str) -> TeamMessage {
    TeamMessage {
        from: from.into(),
        to: to.into(),
        content: "Benchmark message content for throughput testing".into(),
        message_type: "text".into(),
        timestamp: chrono::Utc::now(),
    }
}

fn agent_names(count: usize) -> Vec<String> {
    (0..count).map(|i| format!("agent_{i}")).collect()
}

fn bench_message_bus_send(c: &mut Criterion) {
    let mut group = c.benchmark_group("message_bus_send");

    for agent_count in [3, 9, 20] {
        let names = agent_names(agent_count);
        let bus = MessageBus::new(&names);

        // Subscribe so sends don't fail due to no receivers
        let _receivers: Vec<_> = names.iter().map(|n| bus.subscribe(n).unwrap()).collect();

        group.bench_with_input(
            BenchmarkId::new("send", agent_count),
            &bus,
            |b, bus| {
                b.iter(|| {
                    let msg = make_message("agent_0", "agent_1");
                    black_box(bus.send(msg).unwrap());
                });
            },
        );
    }

    group.finish();
}

fn bench_message_bus_broadcast(c: &mut Criterion) {
    let mut group = c.benchmark_group("message_bus_broadcast");

    for agent_count in [3, 9, 20] {
        let names = agent_names(agent_count);
        let bus = MessageBus::new(&names);

        let _receivers: Vec<_> = names.iter().map(|n| bus.subscribe(n).unwrap()).collect();

        group.bench_with_input(
            BenchmarkId::new("broadcast", agent_count),
            &bus,
            |b, bus| {
                b.iter(|| {
                    let msg = make_message("agent_0", "");
                    black_box(bus.broadcast(msg).unwrap());
                });
            },
        );
    }

    group.finish();
}

fn bench_message_bus_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("message_bus_creation");

    for agent_count in [3, 9, 20, 50] {
        let names = agent_names(agent_count);
        group.bench_with_input(
            BenchmarkId::new("new", agent_count),
            &names,
            |b, names| {
                b.iter(|| black_box(MessageBus::new(names)));
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_message_bus_send,
    bench_message_bus_broadcast,
    bench_message_bus_creation,
);
criterion_main!(benches);
