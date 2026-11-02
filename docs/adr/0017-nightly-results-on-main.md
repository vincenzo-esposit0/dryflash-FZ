<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ADR-0017 — The nightly matrix commits its results to main

- Status: **Accepted**
- Date: 2026-11-02
- Milestone: M6

## Context

BRIEF §10 wants `/compat` published from a nightly run. The site is static and deployed by CI from main, and the page reads `firmware/compat-results.json`. The results have to get from a scheduled job into a deploy. Everything else on main arrives through a
pull request merged by the maintainer.

## Decision

The workflow `nightly.yml` commits that one file directly to main, as `github-actions[bot]`, and starts `ci.yml` on main, which tests and deploys as usual. A push made with `GITHUB_TOKEN` starts no workflow, so it starts it explicitly. The file is written only when a
result changes, or when it is a week old, so a quiet night leaves no commit.

## Alternatives

- **A pull request each time:** keeps every change reviewed, but a pull request opened with `GITHUB_TOKEN` starts no CI, so it would never turn green without another token, and the page would stop being up to date whenever nobody merged.
- **Deploying from the nightly job with no commit:** the next normal deploy would overwrite the results with the old file.
- **Fetching results at run time from elsewhere:** a new moving part for a file that changes rarely.

## Consequences

- A bot writes to main. It only stages that one file, but it holds `contents: write` for the job, on a job that also runs firmware in the emulator and downloads each firmware's latest release. The emulator is the isolation for that code, and a failure of the matrix does not stop results being published.
- If main later gets branch protection, this needs an exception.
- The date on the page can be up to a week old even though the matrix ran last night.
- History gets a commit whenever a result changes. That history is itself a record of when a firmware or the emulator regressed.
