# Compile the application exactly
build: src/main.rs
	@cargo build

test: ## Run the complete test suite
	@cargo test

.PHONY: build test
%.o: %.c
	cc -c $< -o $@
