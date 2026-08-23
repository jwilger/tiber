# BDD and TDD

Specify one independently valuable vertical Gherkin scenario at a time. Start
with a failing observation through the narrowest stable public boundary. Confirm
that it fails for the intended missing behavior before production mutation.
Compilation failure is valid RED only when it specifically demonstrates a
missing required public surface.

Follow the diagnostic one micro-step at a time. Implement only what the current
failure justifies, observe GREEN, and refactor only while green. After each
vertical increment, review for correctness, duplication, and overimplementation
before preserving it.

Do not write broad speculative implementations, weaken tests to obtain GREEN,
or use mocked call choreography as the user-visible acceptance boundary.
