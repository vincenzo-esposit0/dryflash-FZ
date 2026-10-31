<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ADR-0006 — No analytics for now

- Status: **Accepted** (recommendation in the approved M0 plan)
- Date: 2026-10-31

## Decision

No analytics, no cookies, no third-party requests at launch (BRIEF §14 q5). Firmware is loaded
from the user's file or from its original host only.

If usage data becomes necessary later, only a privacy-friendly, cookie-less tool (Plausible or
self-hosted Umami) may be added, through a new ADR, with a visible notice on `/about`.
