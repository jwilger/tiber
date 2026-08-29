---
name: tiber
description: Operate and diagnose the Pi-native Tiber development system, its Rust runtime, model-role routing, and lifecycle gates. Use when setup, doctor, routing, task tracking, workflow, review, or Tiber enforcement is requested in Pi.
---

# Tiber

Use the `tiber_route` tool to request a semantic model role. Rust selects the exact provider/model from Pi's authenticated catalog; do not substitute a model in TypeScript or prose.

Run `/tiber-doctor` to inspect executable and protocol compatibility.

If an operation fails because the Rust runtime is absent, run `npm run runtime:install` from this package. Never bypass a failed Rust decision or implement policy in the adapter.

Additional mature skills will be copied and adapted from the legacy behavioral source as their executable Rust authorities migrate into this repository.
