# Helix Project Governance

This document outlines the governance model, decision-making processes, and RFC (Request for Comments) guidelines for the Helix Project.

## 1. Project Roles

### Core Maintainers
Core Maintainers have write access to the Helix repository and are responsible for the project's health, direction, and release management. They review PRs, triage issues, and drive architectural choices.
* **Lead Maintainer:** Poorna Chandra (@poornachandra24)

### Contributors
Contributors are community members who submit pull requests, improve documentation, report bugs, or engage in discussions. Every contribution is welcome!

---

## 2. Decision-Making Process

Our goal is to build consensus on all technical and project decisions:
* **Minor changes:** Bug fixes, optimizations, documentation, and minor enhancements can be merged by any core maintainer after a standard PR review (at least one approval).
* **Major changes:** Architectural modifications, new core primitives, or additions to the model/adapter APIs require an **RFC process**.

---

## 3. The RFC Process

For major additions, API modifications, or system-level redesigns, we use an RFC (Request for Comments) workflow:

1. **Create an Issue:** Start by opening an issue labeled `RFC: Proposed Feature Name` to describe the problem and get initial feedback.
2. **Write the Proposal:** If the maintainers approve the initial direction, submit a PR containing a markdown file under `docs/rfcs/0000-feature-name.md` detailing:
   * Tagline & abstract
   * Motivation & user impact
   * Detailed design & tech architecture
   * Drawbacks & alternatives considered
3. **Consensus Period:** The PR remains open for at least 7 days to gather community input.
4. **Resolution:** Core maintainers will make the final decision to approve, reject, or request changes to the RFC. Once approved, the RFC PR is merged, and the feature can be implemented.

---

## 4. Code of Conduct Enforcement

All community spaces and governance processes are subject to the [Code of Conduct](CODE_OF_CONDUCT.md). Reports or violations should be sent directly to the core maintainers at **contact@helix.sh**.
