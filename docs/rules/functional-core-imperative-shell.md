# Functional core and imperative shell

Keep deterministic domain decisions referentially transparent. A core function
returns a typed decision, next state, or closed effect description; an adapter
performs I/O and returns a typed observation to the next step.

Use explicit, serializable state machines for work that can suspend, retry,
cancel, or resume. Do not use closures as continuations. Every loop has bounded
attempt, elapsed-time, token, and no-progress limits.

The shell owns filesystem, process, network, memory, inference, and delivery
I/O. It records effects and their observations as durable receipts where the
workflow needs recovery or auditability.
