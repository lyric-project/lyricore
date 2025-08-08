.PHONY: .uv  ## Check that uv is installed
.uv:
	@uv -V || echo 'Please install uv: https://docs.astral.sh/uv/getting-started/installation/'

.PHONY: .pre-commit  ## Check that pre-commit is installed
.pre-commit:
	@pre-commit -V || echo 'Please install pre-commit: https://pre-commit.com/'

.PHONY: install
install: .uv .pre-commit
	uv pip install -U wheel
	uv sync --all-packages --all-extras --all-groups


.PHONY: build-dev
build-dev:
	@rm -f lyricore-py/python/lyricore/*.so
	@cd lyricore-py && uv run maturin develop --uv

.PHONY: build-prod
build-prod:
	@rm -f lyricore-py/python/lyricore/*.so
	@cd lyricore-py && uv run maturin develop --uv --release

.PHONY: build-wasm
build-wasm:
	@echo 'This requires python 3.12, maturin and emsdk to be installed'
	@cd lyricore-py && uv run maturin build --release --target wasm32-unknown-emscripten --out dist -i 3.12
	ls -lh dist

.PHONY: build-whl
build-whl:
	@rm -f lyricore-py/python/lyricore/*.so
#           args: --release --out dist --interpreter ${{ matrix.interpreter || '3.9 3.10 3.11 3.12 3.13 3.14 pypy3.10 pypy3.11' }}
	@#cd lyricore-py && uv run maturin build --release --out dist -i 3.10
	@cd lyricore-py && uv run maturin build --release --out dist -i 3.11
	@#cd lyricore-py && uv run maturin build --release --out dist -i 3.12
	ls -lh lyricore-py/dist

.PYTHON: pytest
pytest:
	@echo 'Running tests...'
	@uv run pytest lyricore-py/tests

.PYTHON: fmt
fmt:
	@uv run ruff format lyricore-py
	@uv run ruff check --select I --fix lyricore-py
	@cd lyricore-py && cargo fmt

.PHONY: clean
clean:
	rm -rf `find . -name __pycache__`
	rm -f `find . -type f -name '*.py[co]' `
	rm -f `find . -type f -name '*~' `
	rm -f `find . -type f -name '.*~' `
	rm -rf .cache
	rm -rf htmlcov
	rm -rf .pytest_cache
	rm -rf *.egg-info
	rm -f .coverage
	rm -f .coverage.*
	rm -rf build
	rm -rf perf.data*
	rm -rf lyricore-py/python/lyricore/*.so