# 📊 Offline Microbenchmarking and Profiling Design

To optimize Helix's local execution subsystems (such as vector quantization, SIMD search, and neural weight updates), we must separate **agent cognitive evaluations** (which are dominated by network latency) from **Rust execution performance** (which is compute- and memory-bound).

This document outlines the design and implementation for local CPU and heap profiling using `criterion` and system-level profiling tools.

---

## 1. Architecture: Cognitive vs. Compute Benchmarks

```
   ┌─────────────────────────────────────────────────────────────┐
   │                       HELIX BENCHMARKS                      │
   └──────────────────────────────┬──────────────────────────────┘
                                  │
         ┌────────────────────────┴────────────────────────┐
         ▼                                                 ▼
┌─────────────────────────────────┐               ┌─────────────────────────────────┐
│     High-Level Agent Evals      │               │      Offline Microbenchmarks    │
│          (/benchmark)           │               │         (cargo bench)           │
├─────────────────────────────────┤               ├─────────────────────────────────┤
│ • Validates LLM tool calling    │               │ • Bypasses networks/APIs        │
│ • Measures task success rates   │               │ • Measures SQLite query times   │
│ • Heavily influenced by network │               │ • Profiles turbovec SIMD search │
│ • Valid for prompt tuning       │               │ • Profiles SONA Micro-LoRA      │
└─────────────────────────────────┘               └─────────────────────────────────┘
```

---

## 2. Implementing `cargo bench` (Criterion Suite)

We introduce a dedicated benchmark target in `Cargo.toml` using `criterion`. This suite generates mock data (synthetic 384-dimensional embeddings) to measure local systems in microseconds.

### Setup in `Cargo.toml`
```toml
[dev-dependencies]
criterion = { version = "0.5", features = ["html_reports"] }

[[bench]]
name = "memory_bench"
harness = false
```

### Benchmark Cases (`benches/memory_bench.rs`)
1. **`turbovec` Query Speed**: Measures cosine similarity search over a 10,000-vector index with SQLite allowlist filtering.
2. **SONA Neural Mutation**: Measures the speed of updating query vectors via the local Micro-LoRA weight update loop.
3. **SQLite Metadata Throughput**: Measures local insertion and lookup rates of document metadata records.

---

## 3. Local Profiling Integration (The Rust Performance Book)

Following the principles in the *Rust Performance Book*, we set up profiles for CPU hot-path tracking and memory allocation tracking.

### A. Heap Allocation Profiling (`dhat`)
We use `dhat` (from Valgrind toolset or as a Rust crate wrapper) to identify temporary allocations (such as `Clone` or `String` transformations) inside query loops.

```bash
# Run with DHAT features enabled to generate dhat-heap.json
cargo bench --bench memory_bench --features dhat-profile
```

### B. CPU Profiling (`cargo-flamegraph`)
We generate interactive SVG flamegraphs to locate CPU cycle bottlenecks in vector quantization math.

```bash
# Generate CPU flamegraph for the memory benches
cargo flamegraph --bench memory_bench
```

---

## 4. Implementation Checklist

- [ ] Add `criterion` and `dhat` to `Cargo.toml`.
- [ ] Create `benches/memory_bench.rs` with mock embedding generation.
- [ ] Add `dhat-profile` cargo feature for conditional heap instrumentation.
- [ ] Document target optimization metrics (P50/P99 latency microsecond baselines).
