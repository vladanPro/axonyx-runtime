# Scoped Head Metadata - 0.8.6 Candidate

Preparation only. Core and Runtime advance to 0.8.6 for the nested layout
metadata fix (#304, #305). Macros and UI are unchanged.

1. Promote the release from `dev` through a `main` PR and wait for its CI.
2. Verify and publish Core first. Once Cargo resolves Core 0.8.6 from
   crates.io, verify and publish Runtime against that published dependency.
3. Update the framework's Runtime submodule, dependency floors, lockfile,
   and CLI/scaffolder pins. Verify the published tooling in a fresh registry-only
   run using the unmodified starter. Check that the site builds and that
   `/metadata` contains the loader-backed title and metadata.

Keep #304 open until the registry-only run passes. A local path dependency
or a starter modification does not satisfy this check.
