# Publication review

The reviewed source snapshot uses the project identity for commit author and committer metadata. Private router environment files, user configuration, raw logs, local screenshots, backup binaries, build output and local diagnostic tools remain outside the publication set. Third-party license and copyright notices remain intact. Public project community links are intentional.

## Corrections

- Subscription refresh diagnostics omit user-supplied subscription names.
- Sharing-service request diagnostics omit client addresses and user-controlled paths and queries.
- Default configuration and file-watch diagnostics omit personal paths.
- The optional deployment tool verifies an existing SSH host key and no longer changes the active node as a verification step.
- Formatting and first-party static-analysis findings were corrected.
- UDP packet IDs use checked compare/exchange, preserving exhaustion behavior across Rust versions without a deprecated API. Locale-changing unit tests serialize access to the shared language setting.

## Verification boundaries

Core/CLI and integration tests exercise local synthetic data, configuration ownership, subscription persistence, bounded queues, HTTP framing, UDP lifecycle and recovery journals. Merlin fault scenarios, RPC isolation and browser transport checks are separate from extended live-router soak testing. Regression tests and self-signed localhost TLS fixtures contain synthetic data and are not deployed runtime credentials.

Desktop source checking uses GPUI runtime shaders on machines without the Xcode Metal compiler component; this does not establish successful packaging of a release app. Windows and other platform builds require their own runners. Existing dependency warnings are not equivalent to first-party static-analysis failures.

Rewriting branches and deleting releases, tags, workflow runs, artifacts and caches removes the repository-controlled historical entry points. GitHub-controlled pull-request refs, cached commit views, external forks and previous downloads cannot be erased by a normal force push. GitHub Support must assess removal of any residual sensitive historical content.
