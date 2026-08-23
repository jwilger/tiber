# Functional core and imperative shell

Domain code is deterministic and side-effect free. It accepts semantic values
and returns events, closed typed effects, or typed failures. It does not access
the filesystem, network, process environment, Git, models, clocks, randomness,
or UI.

Adapters parse external data once and interpret effects. Application services
sequence decisions and effects without embedding domain policy. Persist intent
before consequential effects and validate observations before recording a
receipt. Recovery reconciles unresolved intent rather than blindly replaying.

Do not introduce generic mutable aggregates, callback-bearing workflow nodes,
or generic effect escape hatches. A command folds only the facts needed for its
decision.
