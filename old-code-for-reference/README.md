# Legacy source reference

This directory preserves first-party source from the retired `ai-plugins`
marketplace while Tiber absorbs the capabilities it needs as native services.
It is not a Cargo workspace member, is not built by Tiber CI, and ships in no
Tiber release.

The retained source is organized by the former marketplace component:

- `tiber-tasks/` is the former Tiber task-board implementation.
- `development-workflow/` is the former Development Discipline workflow
  implementation.

The owner has authorized reuse and relicensing of this first-party material for
Tiber. New native code belongs under `crates/`, is licensed under Apache-2.0,
and must follow Tiber's current architecture and EventCore rules. The legacy
Cargo manifests intentionally retain their historical metadata as provenance;
they are not a licensing statement for Tiber releases.

Do not add product dependencies on this directory. Port a bounded behavior with
its durable-event and public-contract tests, then remove the reference material
once every needed behavior has a native home.
