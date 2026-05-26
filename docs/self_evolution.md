# Self-Evolution Loop and Security Gates

Helix features a secure, metrics-driven self-evolution loop. This subsystem allows the engine to analyze its execution telemetry, propose optimizations to its own Rust codebase, verify safety and regression gates, and apply updates upon human confirmation.

---

## 1. Safety Policies & Immutable Gates

To prevent recursive model degradation, security exploits, or system access escalation, Helix enforces a strict multi-tier security filter before compiling any proposed patch:

1.  **Immutable Lock (`.evolution-lock`)**: Certain core modules (such as `src/evolution/`, `src/metrics/`, or the evolutionary gates themselves) are locked. The model is physically blocked from proposing changes targeting these files.
2.  **Unsafe Operations Ban**: Any proposed patch containing the word `unsafe` or adding dangerous system API calls (such as spawning raw shells outside our confirmation gates) is instantly discarded.
3.  **Compilation Gate**: The engine executes `cargo check` and `cargo test` in a sandboxed target configuration. If compilation fails or compiler warnings are introduced, the patch is rejected.
4.  **No-Regression Gate**: The engine executes the headless Benchmark Suite (`benchmarks/`) and compares telemetry metrics (turn latency, token count, healer corrections, tool success rates) against `baseline.json`. If performance degrades, the engine automatically issues a `git revert` to roll back the changes.
5.  **Human Authorization Gate (or --auto-approve override)**: By default, Helix prompts the user for `/approve` or `/reject` to apply or discard the diff. However, if trust in the automated test gates is high, operators can bypass this manually using `/evolve --auto-approve` to let Helix self-apply changes immediately upon successful validation.

---

## 2. Evolution Subsystem Sequence Diagram

The sequence diagram below displays the self-evolution lifecycle from log analysis to codebase mutation:

```mermaid
sequenceDiagram
    autonumber
    actor User as User / Operator
    participant CLI as src/main.rs (REPL)
    participant Evolve as src/evolution/ (EvolutionEngine)
    participant Metrics as src/metrics/ (MetricsCollector)
    participant LLM as Evolution LLM Agent
    participant Cargo as Rust Toolchain (cargo)
    participant Suite as Benchmark Suite

    User->>CLI: Type /evolve command
    activate CLI
    
    CLI->>Metrics: Read turn telemetry & session logs
    Metrics-->>CLI: Return bottlenecks & metric reports
    
    CLI->>Evolve: propose_improvement(session_logs, source_code)
    activate Evolve
    Evolve->>LLM: Prompt with logs & target code files
    LLM-->>Evolve: Return proposed unified diff
    
    %% Security Verification
    Evolve->>Evolve: Run Security Checks
    Note over Evolve: 1. Scan target files against .evolution-lock<br/>2. Block 'unsafe' keyword additions<br/>3. Verify no destructive system tools
    
    alt Security Checks Failed
        Evolve-->>CLI: Reject proposal (Report Security Violation)
        CLI-->>User: Display security failure reason
    else Security Checks Passed
        Evolve->>Evolve: Temporarily apply diff to source tree
        
        %% Compilation Check
        Evolve->>Cargo: cargo check / cargo test
        Cargo-->>Evolve: Return compiler status
        
        alt Compilation Fails
            Evolve->>Evolve: Rollback diff (git checkout)
            Evolve-->>CLI: Reject proposal (Compilation failure)
            CLI-->>User: Display compilation failure logs
        else Compilation Succeeds
            %% Benchmark Verification
            Evolve->>Suite: Run benchmark tests
            Suite-->>Evolve: Return telemetry metrics
            
            Evolve->>Evolve: Compare against baseline.json
            
            alt Performance Regressed
                Evolve->>Evolve: Rollback diff (git checkout)
                Evolve-->>CLI: Reject proposal (Metric regression)
                CLI-->>User: Display regression metrics
            else Performance Within Baseline
                Evolve-->>CLI: Report successful evolution proposal
                deactivate Evolve
                CLI-->>User: Print unified diff & request /approve or /reject
            end
        end
    end
    deactivate CLI

    %% Human-in-the-loop Phase
    alt User Rejects
        User->>CLI: /reject [reason]
        CLI->>Evolve: discard_evolution()
        CLI-->>User: Evolution discarded
    else User Approves
        User->>CLI: /approve
        activate CLI
        CLI->>Evolve: commit_evolution()
        Note over Evolve: Hard commit patch to git history
        Evolve-->>CLI: Return commit success
        CLI-->>User: Display success banner (Helix code updated!)
        deactivate CLI
    end
```
