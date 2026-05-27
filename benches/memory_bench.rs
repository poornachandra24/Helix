#[cfg(any(target_os = "linux", target_os = "macos"))]
extern crate blas_src;

use criterion::{criterion_group, criterion_main, Criterion};
use turbovec::IdMapIndex;
use ruvector_sona::{SonaConfig, SonaEngine};
use rusqlite::Connection;
use rand::Rng;

fn bench_turbovec_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("turbovec_search");
    
    // Setup index with 1,000 vectors of dimension 384
    let mut index = IdMapIndex::new(384, 4).unwrap();
    let mut rng = rand::thread_rng();
    
    let mut ids = Vec::new();
    for i in 0..1000 {
        let mut vector = vec![0.0f32; 384];
        for val in vector.iter_mut() {
            *val = rng.gen_range(-1.0..1.0);
        }
        let id = i as u64;
        index.add_with_ids(&vector, &[id]).unwrap();
        ids.push(id);
    }
    
    // Generate a query vector
    let mut query = vec![0.0f32; 384];
    for val in query.iter_mut() {
        *val = rng.gen_range(-1.0..1.0);
    }

    // Benchmark search without allowlist
    group.bench_function("search_all", |b| {
        b.iter(|| {
            let _ = index.search(&query, 5);
        })
    });

    // Benchmark search with allowlist (simulating active workspace isolations)
    let allowlist: Vec<u64> = ids.iter().copied().take(100).collect();
    group.bench_function("search_with_allowlist_100", |b| {
        b.iter(|| {
            let _ = index.search_with_allowlist(&query, 5, Some(&allowlist));
        })
    });
    
    group.finish();
}

fn bench_sona_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("sona_neural_adaptation");
    
    let sona_config = SonaConfig {
        hidden_dim: 384,
        embedding_dim: 384,
        ..Default::default()
    };
    let sona_engine = SonaEngine::with_config(sona_config);
    let mut rng = rand::thread_rng();
    
    let mut query = vec![0.0f32; 384];
    for val in query.iter_mut() {
        *val = rng.gen_range(-1.0..1.0);
    }

    group.bench_function("apply_micro_lora", |b| {
        let mut optimized = vec![0.0f32; 384];
        b.iter(|| {
            sona_engine.apply_micro_lora(&query, &mut optimized);
        })
    });

    group.bench_function("trajectory_update", |b| {
        b.iter(|| {
            let mut trajectory = sona_engine.begin_trajectory(query.clone());
            trajectory.add_step(vec![0.0f32; 384], vec![], 0.85);
            trajectory.set_model_route("gpt-oss");
            sona_engine.end_trajectory(trajectory, 0.85);
            let _ = sona_engine.tick();
        })
    });

    group.finish();
}

fn bench_sqlite_metadata(c: &mut Criterion) {
    let mut group = c.benchmark_group("sqlite_metadata");
    
    let db = Connection::open_in_memory().unwrap();
    db.execute(
        "CREATE TABLE IF NOT EXISTS memory_metadata (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            text TEXT NOT NULL,
            file_path TEXT,
            workspace_path TEXT NOT NULL,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )",
        [],
    ).unwrap();
    
    // Pre-populate with 1,000 records
    for i in 0..1000 {
        db.execute(
            "INSERT INTO memory_metadata (text, file_path, workspace_path) VALUES (?, ?, ?)",
            rusqlite::params![
                format!("Some code document content representing function number {}", i),
                format!("src/module/sub_{}.rs", i),
                "/home/user/workspace/project"
            ]
        ).unwrap();
    }
    
    group.bench_function("query_workspace_ids", |b| {
        b.iter(|| {
            let mut stmt = db.prepare(
                "SELECT id FROM memory_metadata WHERE workspace_path = ?"
            ).unwrap();
            let ids: Vec<u64> = stmt
                .query_map(["/home/user/workspace/project"], |row| row.get(0))
                .unwrap()
                .filter_map(Result::ok)
                .collect();
            assert_eq!(ids.len(), 1000);
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_turbovec_search,
    bench_sona_operations,
    bench_sqlite_metadata
);
criterion_main!(benches);
