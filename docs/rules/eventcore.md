# EventCore modeling

EventCore records durable business facts; it is not an aggregate framework.
Commands name a business-domain intent and fold their own minimal decision state
from the relevant fact history. That state contains only what the command needs
to decide whether and which facts to emit.

Do not use aggregate objects, mutable shared write models, or a generic
whole-session replay state as command authority. Keep read projections separate
from command folds. Evolve facts compatibly when existing history must replay.

Every shipping command model uses EventCore's checked-model facilities. The
model graph and tests must consume command-origin and field provenance with no
unreviewed warnings.
