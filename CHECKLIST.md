# Helix Project Launch Checklist

## 1️⃣ Repository & Licensing
- [x] Add `LICENSE` (MIT) and ensure it is in the repo root.
- [x] Add `CODE_OF_CONDUCT.md`.
- [x] Add `CONTRIBUTING.md` with guidelines for fork‑and‑PR workflow.
- [x] Verify `Cargo.toml` contains correct package metadata (name, version, repository URL, license).

## 2️⃣ CI / Automation
- [x] Set up GitHub Actions workflow:
  - Build on Linux, macOS, Windows.
  - Run `cargo fmt -- --check` and `cargo clippy -- -D warnings`.
  - Run `cargo test --all-features`.
  - Run benchmark suite and publish results as an artifact.
- [x] Enable Dependabot for Cargo updates.
- [x] Add a security‑gate check that runs on every PR (static analysis, cargo audit).

## 3️⃣ Documentation
- [x] Create `README.md` with:
  - One‑sentence tagline.
  - Quick‑start (clone → `cargo run`).
  - Feature matrix vs. competitors.
  - Link to the full docs site.
- [x] Generate docs with `cargo doc --no-deps` and publish to GitHub Pages (or docs.rs).
- [x] Add `docs/` folder with:
  - Architecture overview.
  - Plugin development guide.
  - Edge‑deployment guide.
- [x] Add `CHANGELOG.md` following Keep a Changelog format.

## 4️⃣ Release Assets
- [x] Build release binaries for Linux, macOS, Windows (static linking where possible).
- [x] Create a minimal Dockerfile (`FROM scratch` + binary) and push to Docker Hub.
- [ ] Tag releases with semantic versioning (`vX.Y.Z`).
- [x] Attach binaries and Docker image digest to the GitHub Release.

## 5️⃣ Community & Governance
- [ ] Set up Discord/Matrix community server and add invite link to README.
- [ ] Enable GitHub Discussions for Q&A and ideas.
- [x] Draft a `GOVERNANCE.md` describing:
  - Core maintainers.
  - RFC process for major changes.
- [ ] Add a `FUNDING.yml` with OpenCollective, GitHub Sponsors, Liberapay.

## 6️⃣ Security & Privacy
- [x] Document the static security gate and its rules.
- [x] Publish a security‑bounty policy (e.g., via HackerOne or GitHub Security Advisories).
- [x] Ensure all dependencies are audited (`cargo audit`) and no unsafe code is introduced without review.

## 7️⃣ Marketing Materials
- [ ] Design a simple logo (geometric “H” resembling a helix).
- [ ] Create a hero GIF/video showing the REPL → sandboxed tool execution → schema healing.
- [ ] Prepare a one‑pager PDF with tagline, pillars, and target audience.
- [ ] Draft a blog post announcing the open‑source launch.

## 8️⃣ Roadmap & Future Work
- [x] Publish a `ROADMAP.md` with short‑, mid‑, and long‑term milestones.
- [x] Highlight upcoming features: WASM sandbox, distributed index, UI dashboard.

---

**Ready to ship!** Once every item above is checked, the Helix project will be fully launch‑ready, community‑friendly, and positioned as the go‑to tiny, self‑evolving AI agent.
