# Helix Terminal UI Design

This document describes the terminal UI system used in `src/cli/repl.rs` and `src/core/engine.rs`.

---

## 1. Dynamic Layout

All UI element widths are computed at runtime from the terminal column count:

```rust
let terminal_width = console::Term::stdout()
    .size_checked()
    .map(|(_, cols)| cols as usize)
    .unwrap_or(80);

let content_width = terminal_width.saturating_sub(8).clamp(50, 110);
```

- **`terminal_width`**: Actual terminal columns, with fallback to 80.
- **`content_width`**: The writable area inside a box. Clamped to `[50, 110]` to prevent layouts that are too narrow or too wide on huge displays.
- **Outer box width**: `content_width + 8` characters (2 indent + 1 border + 1 space + content + 1 space + 1 border).

---

## 2. Box Math

Given `content_width = W`:

| Element | Width formula |
|---------|--------------|
| Top border dashes | `W - title.len()` |
| Content row padding | `format!("  {:W$}  ", line)` → `W + 4` chars |
| Bottom border dashes | `W + 4` |
| Outer box total | `W + 8` |

### Rounded corners

All boxes use Unicode rounded box-drawing corners to avoid the hard "computer terminal" feel of square corners. Indentation is exactly 2 spaces:

```
  ╭── Label ────────────────────────╮
  │  content here                   │
  ╰────────── [Elapsed: X.XXs] ─────╯
```

### Avoiding ANSI color bleed on headers

The top border is printed as **separate `print!` calls** — never as a single nested `format!`. This prevents the inner `\x1b[0m` reset from the styled label from stripping the border color applied to the trailing dashes:

```rust
// CORRECT — three separate calls
print!("  ");
print!("{}", border_color.apply_to("╭── "));
print!("{}", header_color.apply_to(title));
println!("{}", border_color.apply_to(format!(" {}╮", "─".repeat(dashes_count))));

// WRONG — inner reset bleeds into trailing dashes
println!("{}", border_color.apply_to(format!("╭── {} {}╮",
    header_color.apply_to(title),   // emits \x1b[0m, resets border_color
    "─".repeat(dashes_count)        // now renders in terminal default white
)));
```

---

## 3. Color Tokens

### Startup banner (`src/cli/helpers.rs :: print_banner`)

| Element | `color256` | Description |
|---------|-----------|-------------|
| Logo line 1 | 81 | Soft cyan |
| Logo line 2 | 75 | Sky blue |
| Logo line 3 | 111 | Pastel blue |
| Logo line 4 | 147 | Soft lavender |
| Logo line 5 | 183 | Soft violet/pink |
| Box border | 240 | Subtle dark grey |
| Label keys | 248 | Light grey |
| Label values | 253 | Bright white |
| Status indicator `●` | 46 | Matrix green |
| `/help` hint | 51 | Cyber cyan |

### User Input box (`src/cli/repl.rs`)

| Element | Color | Description |
|---------|-------|-------------|
| Border / corners | Blue | Standard blue |
| Header label `You` | Blue (Bold) | Bold blue |
| Body text | Default | Terminal default text |

### Agent Response box (`src/cli/repl.rs`)

| Element | Color | Description |
|---------|-------|-------------|
| Border / corners | Yellow | Standard yellow |
| Header label `Helix` | Yellow (Bold) | Bold yellow |
| Elapsed timer label | Yellow (Bold) | Bold yellow (same as header) |

### Spinner (`src/cli/repl.rs`)

| Element | Color | Description |
|---------|-------|-------------|
| Spinner frame | Cyan (Bold) | Cyber cyan |
| Spinner suffix text | Dimmed | Dimmed/grey text |

### Post-response footnotes (`src/core/engine.rs`)

| Element | Color | Description |
|---------|-------|-------------|
| Memory bullet `◆` | Cyan | Soft cyan |
| Memory no-match bullet `◇` | Dimmed | Dim grey |
| Micro-LoRA hint | Dimmed | Dim grey |
| Tool dispatch `⦿` | Yellow | Amber/yellow |
| Tool completed `✓` | Green | Matrix green |
| Loop label `· loop N/20` | Dimmed | Subtle dark grey |
| SONA label `· [sona]` | Dimmed | Mid grey |
| SONA quality (good > 0.8) | Green | Terminal green |
| SONA quality (ok > 0.5) | Yellow | Terminal yellow |
| SONA quality (poor) | Red | Terminal red |

---

## 4. Word Wrapping & Markdown Rendering

We use a hybrid approach to balance streaming response indicators with high-fidelity markdown rendering:

- **Buffer and Spin**: During token generation, tokens are buffered into a complete response string while displaying an async tick spinner (`⠋ thinking...`).
- **Markdown Formatting**: When token generation completes, the full response is rendered via `termimad` (using a default responsive terminal skin) to wrap text to `content_width` and handle rich formatting (bold, lists, headers, code blocks).
- **ANSI-Aware Border Alignment**: The rendered lines (containing ANSI formatting escape sequences) are parsed line-by-line. We use `console::strip_ansi_codes` to determine each line's actual display width, ensuring spaces are padded correctly and the box's borders stay aligned.

---

## 5. Post-Response Telemetry Format

All post-box telemetry is sent via the `\x1b[T` token prefix through the streaming channel and printed *after* the response box closes. The intended visual layout is:

```
  ╰──────────────── [Elapsed: 6.18s] ─────────────────╯

  ◆ Memory: Found 5 relevant workspace memories (adapted via Micro-LoRA)
  ⦿ Tool: dispatching 1 → bash
  ✓ 'bash' completed (219 bytes)
  · loop 1/20
  · [sona] quality 0.95
```

**Rules for all telemetry messages:**
- Indent with 2 spaces (`"  "`).
- Use a single character bullet/symbol prefix, not box-drawing characters (no `│`, `└`, `┌`).
- No trailing horizontal dashes or separators — these visually anchor to nothing after the box has closed.

---

## 6. Agent Loop Counter

The engine can run up to `max_iterations` (default: 20) tool-use iterations per turn. Each iteration emits:

```
  · loop N/20        (dim footnote after the box)
  Working (loop N/20)...  (spinner suffix during thinking)
```

The counter gives developers visibility into agentic behavior without overwhelming regular users with implementation detail.

---

## 7. SONA Trajectory Footnote

After every completed turn, the SONA engine emits a quality score representing the turn's effectiveness (based on steps taken, healer retries, tool success rate):

```
  · [sona] quality 0.95
```

This is always present after every response. It is intentional — SONA records a trajectory for *every* turn to continuously improve Micro-LoRA weights for semantic memory retrieval. The quality score is color-coded: green (> 0.8), yellow (> 0.5), red (≤ 0.5).
