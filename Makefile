.PHONY: build check unit integration acceptance benchmark fixtures secret-check local-up local-down

build:
	cargo build --locked --workspace
	npm run build

check:
	cargo fmt --all -- --check
	cargo clippy --locked --workspace --all-targets -- -D warnings
	npm run check
	$(MAKE) secret-check

unit:
	cargo test --locked --workspace
	python3 tests/acceptance/run.py --suite fixtures

fixtures:
	python3 tests/acceptance/run.py --suite fixtures

integration:
	python3 tests/acceptance/stack.py run -- python3 tests/acceptance/run.py --suite integration

acceptance:
	python3 tests/acceptance/stack.py run -- python3 tests/acceptance/run.py --suite acceptance

# Requires an explicit synthetic batch and credential file; no default credentials.
benchmark:
	python3 benchmarks/http.py $(BENCHMARK_ARGS)

secret-check:
	python3 tests/acceptance/secret_check.py

local-up:
	python3 tests/acceptance/stack.py up

# Pass STATE=local/acceptance/run-.../state.json from local-up.
local-down:
	@test -n "$(STATE)" || (echo "STATE is required"; exit 1)
	python3 tests/acceptance/stack.py down --state "$(STATE)"
