# Semantic types

Parse external representations once at an explicit boundary. Domain values with
identity, units, authority, format, or validity constraints use a newtype,
smart constructor, refined type, or closed alternative that makes invalid state
unconstructable.

Do not wrap a primitive that has no domain meaning merely for decoration.
Reject malformed external input as a typed error at the boundary instead of
re-validating arbitrary strings throughout the core.
