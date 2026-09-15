# Repository agent instructions

## Commit and push

- Never run `git push` directly in this repository.
- After creating the requested commit or commits, run `./git-push.sh` in place of `git push`.
- Pass normal push arguments through the wrapper when needed, for example `./git-push.sh origin master`.
- The wrapper checks LiteLLM's latest model-price source before pushing. When the source changed, it derives and audits `pricing/model-prices.json`, creates a separate `chore(pricing): sync LiteLLM model prices` commit, and then pushes all pending commits.
- Do not stage unrelated working-tree changes merely to make the wrapper run. It intentionally commits only the generated price catalog.
