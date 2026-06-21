# Local Semantic Memory Architecture

Helix implements an offline, high-performance semantic memory store to persist and retrieve relevant past workspace interactions. This allows the agent to maintain context across sessions without bloating the active LLM context window.

---

## 1. Subsystem Components

*   **SQLite Metadata Store & FTS5 BM25 Search**: A local SQL database (`memory_meta.db`) that stores text snippets, associated files, and workspace directories. Uses SQLite's FTS5 extension to perform full-text BM25 search alongside vector queries.
*   **turbovec Quantized Index**: A high-performance vector index (`memory_index.tvim`) utilizing **4-bit Lloyd-Max Scalar Quantization** for 8x memory compression. Vectors are matched using SIMD-accelerated cosine similarity.
*   **fastembed-rs ONNX Embeddings**: Automatically loads and caches the `BAAI/bge-small-en-v1.5` text embedding model (384 dimensions) completely offline using ONNX Runtime.
*   **BM25 Hybrid Retrieval**: Combines semantic embeddings with lexical FTS5 queries using Reciprocal Rank Fusion (RRF) to retrieve highly relevant context matches.
*   **Post-Mortem Reflective Memory**: Automatically summarizes and index-serializes the user-agent interaction session upon exit, feeding into long-term recall cache.
*   **Workspace Filtering & Allowlist**: To isolate workspaces, queries are filtered by mapping workspace row IDs from SQLite directly into `turbovec`'s search allowlist. This prevents memory leakage between projects.

---

## 2. Memory Subsystem Data Flow

The diagram below details the data flow during query search and turn-level storage:

```mermaid
graph TD
    subgraph Search Phase
        QueryInput[User Query] --> EmbedModel[fastembed: BAAI/bge-small-en-v1.5]
        EmbedModel --> QueryVec[Query Vector: 384 dimensions]
        
        Workspace[Active Workspace Path] --> SQLiteMeta[SQLite metadata]
        SQLiteMeta --> sqlite_ids[Find metadata IDs for workspace]
        sqlite_ids --> AllowlistCheck[Filter out non-indexed IDs]
        AllowlistCheck --> ActiveAllowlist[Active Allowlist]
        
        QueryVec & ActiveAllowlist --> SIMDSearch[turbovec: search_with_allowlist]
        SIMDSearch --> RawMatches[Top matches & scores]
        RawMatches --> QuerySQLite[Lookup matching text snippets in SQLite]
        QuerySQLite --> Matches[Return MemoryMatches]
    end

    subgraph Insertion Phase
        NewText[New Text Snippet] --> InsertSQLite[Insert metadata row in SQLite]
        InsertSQLite --> NewID[Retrieve generated row ID]
        NewText --> EmbedModelInsert[fastembed: BAAI/bge-small-en-v1.5]
        EmbedModelInsert --> Vector[384 float vector]
        Vector & NewID --> AddIndex[turbovec: add_with_ids]
        AddIndex --> Persist[Serialize index to .tvim]
    end
```

---

## 3. Temporal Sequence Diagram

This sequence diagram displays the step-by-step operations executed during a standard user interaction turn in the REPL:

```mermaid
sequenceDiagram
    autonumber
    actor User as User / REPL
    participant Engine as src/engine.rs (Engine)
    participant Memory as src/memory.rs (HelixMemoryEngine)
    participant fastembed as fastembed (ONNX Embedder)
    participant turbovec as turbovec (Vector Index)
    participant SQLite as SQLite Database
    participant LLM as Active LLM Model

    User->>Engine: Send prompt / input turn
    activate Engine
    
    %% Retrieval Hook
    Engine->>Memory: search(input, workspace_path, limit=5)
    activate Memory
    Memory->>fastembed: embed(input)
    fastembed-->>Memory: Return query vector
    
    Memory->>SQLite: Get IDs matching active workspace
    SQLite-->>Memory: Return sqlite_ids
    
    Memory->>Memory: Filter allowed_ids (exist in turbovec)
    
    Memory->>turbovec: search_with_allowlist(query_vec, allowed_ids)
    turbovec-->>Memory: Return matching vector IDs & scores
    
    Memory->>SQLite: Get text for matching IDs
    SQLite-->>Memory: Return text snippets
    
    Memory-->>Engine: Return Vec<MemoryMatch>
    deactivate Memory

    Engine->>Engine: Enrich system prompt with retrieved memories
    
    %% Turn Execution
    Engine->>LLM: Send turn (enriched system prompt + input)
    LLM-->>Engine: Return turn response text
    
    %% Save Memory Hook
    Engine->>Memory: insert("User: ... \nAssistant: ...", workspace_path)
    activate Memory
    Memory->>SQLite: Insert metadata (text, workspace_path)
    SQLite-->>Memory: Return row_id
    
    Memory->>fastembed: embed(memory_text)
    fastembed-->>Memory: Return embedding vector
    
    Memory->>turbovec: add_with_ids(vector, row_id)
    Memory->>Memory: persist() index to disk
    Memory-->>Engine: Return Ok(())
    deactivate Memory

    Engine-->>User: Return response to REPL
    deactivate Engine
```

---

## 4. Memory Footprint and Resource Verification

Helix is optimized to maintain a highly compact memory profile suitable for resource-constrained environments (such as a 256 MiB container limit).

### 📊 Resource Baseline and Scaling Profile

| Component | Approx. Memory (RSS) | Description / Impact |
|---|---|---|
| **Static Binary & Runtime** | ~5–9 MiB | Compiled release executable and mapped system libraries (`libc`, `libpthread`). |
| **ONNX Runtime Engine** | ~15–20 MiB | Shared libraries and thread-pool execution structures. |
| **FastEmbed (BGE-small-v1.5)** | ~45–50 MiB | ONNX weights mapped into physical memory. |
| **Active Runtime Engine (RSS)** | **~227 MiB** | Measured resident set size of the process on startup with 17 active threads. |
| **Quantized Index (turbovec)** | **~3.2 MiB** per 100k vectors | 4-bit quantization reduces 384-dim float vectors to just 24 bytes each. |
| **Metadata Cache (SQLite)** | **~0.5 MiB** per 100k rows | SQL database pages storing prompt snippets and workspace links. |

### 🔍 Thread Pool Scaling & RSS Explanation
Upon startup, Helix initializes **17 OS threads**. This is driven by ONNX Runtime (`ort` via `fastembed-rs`) configuring its parallel tensor execution pool to match the host machine's logical core count. 
While the raw model files on disk occupy only ~45–50 MiB, the total resident set size (VmRSS) baseline is ~227 MiB. This baseline includes:
1. ONNX pre-allocated tensor arenas.
2. Thread stacks (active pages).
3. Glibc memory allocation arenas.
4. Mapped shared object (.so) segments.

### 🛠️ Empirical Verification
To verify this memory footprint directly on a running Linux instance:

```bash
# Start Helix in the background, run status, and exit
(echo "/status"; sleep 5; echo "/exit") | ./target/release/helix &
PID=$!
sleep 1.5

# Check RSS, virtual size, and thread counts
cat /proc/$PID/status | grep -E "VmRSS|VmSize|VmPeak|Threads"
```

---

## 5. Lexical & Semantic Hybrid Search (BM25 + Vector)

To match exact technical vocabulary (such as function names, API keys, error codes, and config tags) that dense vector embeddings might dilute, Helix implements a hybrid retrieval model:

1.  **Lexical Query (BM25)**: An FTS5-backed virtual table index in SQLite performs full-text search. Term occurrences are scored using the Okapi BM25 algorithm.
2.  **Semantic Query (Cosine)**: Dense 384-dimensional vectors are evaluated in the `turbovec` index using SIMD-optimized cosine similarity.
3.  **Reciprocal Rank Fusion (RRF)**: Merges the ranked outputs of both search stages, ensuring items matching either exact keyword tokens or broad conceptual intent rise to the top.

---

## 6. Post-Mortem Reflective Memory

Upon termination of a REPL session (via `/exit` or normal interrupt), the harness triggers an autonomous compilation pipeline:

```mermaid
graph LR
    History[Session History Buffer] --> Summarize[LLM Summarization Module]
    Summarize --> SummaryMarkdown[Markdown Session Summary]
    SummaryMarkdown --> Vectorize[fastembed BGE-small Embedder]
    SummaryMarkdown --> StoreSQL[SQLite Metadata Indexer]
    Vectorize & StoreSQL --> Index[Local Memory Index]
```

- **Summarization**: The engine passes the active session history to the LLM cloud/local engine, asking it to write a structured summary highlighting the goals achieved, technical decisions made, and outstanding issues.
- **Indexing**: The generated summary is written to `<data_dir>/memory/sessions/` and automatically embedded and indexed in SQLite and `turbovec`. Future sessions starting in the same workspace automatically inherit these key learnings.

