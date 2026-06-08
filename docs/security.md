# Helix Security & Privacy Policy

This document outlines the security architecture, the automated security checks built into our CI/CD pipeline, and the guidelines for reporting security vulnerabilities.

## 1. Automated Security Gates (CI/CD)

Every pull request and commit to the `main` branch undergoes automated security testing as part of our GitHub Actions workflow:

* **Static Application Security Testing (SAST):** We use cargo lints and `clippy` to enforce memory safety and code quality rules.
* **Dependency Audits:** We run `cargo audit` on every build to scan for known security vulnerabilities in all third-party crates defined in `Cargo.lock`.
* **Vulnerability Scanning:** Dependency trees are checked for licensing violations and deprecations.

---

## 2. Secure Coding Guidelines

To minimize the project's attack surface, Helix adheres to the following secure coding principles:

* **Zero Unsafe Rust:** We do not allow `unsafe` blocks in Helix source code unless absolutely necessary and thoroughly reviewed by at least two core maintainers.
* **Minimal Dependencies:** We deliberately audit and prune unused dependencies to prevent supply-chain attacks.
* **Confirming Local Actions:** The Helix REPL prompts the user with an explicit interactive confirmation before executing any local terminal command or tool action that could affect the system state.

---

## 3. Vulnerability Reporting & Bounty Policy

If you discover a security vulnerability in Helix, please report it responsibly:

* **Do not open a public issue.**
* Send a detailed report to **security@helix.sh** with:
  * A description of the vulnerability.
  * Steps or a proof-of-concept script to reproduce the issue.
  * Potential impact.
* The maintainers will acknowledge your report within 48 hours and work on a patch.
* Once resolved, credit will be given in the release notes and advisory.
