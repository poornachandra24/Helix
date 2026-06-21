# Helix Skill Registry

Helix includes a lightweight, local **Skill Registry** subsystem that allows users to define custom, domain-specific agent instructions, workflows, or rules. 

Rather than overloading the baseline system prompt or requiring manual copy-paste at the start of every session, you can define these guidelines as modular text files.

---

## How It Works

1. **Active Folder Scan**: On startup, Helix scans the `skills/` subdirectory within its data directory (e.g., `~/.local/share/helix/skills/` on Linux, `~/Library/Application Support/com.helix.helix/skills/` on macOS, or `%USERPROFILE%\AppData\Roaming\helix\helix\data\skills\` on Windows).
2. **Dynamic Compilation**: Helix filters for files with `.md` or `.txt` extensions, reads their contents, and formats them into distinct modular sections.
3. **System Prompt Injection**: If skills are present, Helix automatically appends them to the base system prompt under the header:
   ```text
   You have the following domain-specific skills available:
   
   --- Skill: [filename] ---
   [File Content]
   ```

---

## Adding Custom Skills

To add a new skill to Helix:

1. Locate or create the `skills` directory in your Helix data folder (e.g., `~/.local/share/helix/skills/` on Linux).
2. Create a new markdown (`.md`) or text (`.txt`) file. For example, `rust_guidelines.md`:
   ```markdown
   Always use standard library error types instead of custom error enums when writing small CLI helpers.
   Avoid importing external macro crates unless explicitly requested.
   ```
3. Run or restart the Helix shell. The new guidelines will be dynamically injected into the model's system context.

---

## Best Practices

* **Keep it Concise**: Focus each skill file on a single workflow, framework, or coding standard (e.g., `react_formatting.md`, `postgres_indexes.txt`).
* **Clean Formatting**: Use standard markdown structure (headers, lists) inside your skill files so the model can easily parse constraints.
