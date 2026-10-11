# Rust Coding Rules for AI Agents

Rules for every AI agent that writes, edits, reviews, or generates Rust code.
They come from the Rust Style Guide, the Rust API Guidelines, Clippy, and common community practice.

**How to read the keywords**

- **MUST / MUST NOT**: strict rule. Break it only if the user clearly says so.
- **SHOULD / SHOULD NOT**: default rule. Break it only with a good reason, and say why in a short comment or in your reply.
- **MAY**: optional.

**Priority order (when two rules clash, the higher one wins):**

1. Correctness and safety
2. The user's explicit instructions
3. Idiomatic Rust
4. Official style (`rustfmt`)
5. API Guidelines
6. Readability and maintainability

---

## 0. Before You Write Code

- Read the existing code first. Match its style, error types, and crate choices.
- Check `Cargo.toml` for the edition, the MSRV (`rust-version`), and the existing dependencies.
- Know if the code is a **library** or an **application (binary)**. Some rules differ (see section 5).
- Know if the code is **sync or async**, and which runtime is used.
- Do not invent APIs. If you are not sure a function, method, or crate feature exists, say so or check the docs. Do not guess.
- Do not add a new dependency unless it is really needed. When you add one, say why, and use the version style the project already uses.
- Keep changes small. Do not reformat or rewrite code that has nothing to do with the task.

---

## 1. Mandatory Tooling

Code MUST pass all of these:

```text
cargo fmt --check
cargo fmt --check -- --config newline_style=Unix
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo test --doc
cargo doc --no-deps   # with no warnings
```

- Target the latest stable Rust and the project's edition (2024 for new projects).
- Do not use unstable or nightly features unless the user asks.
- Respect the project's MSRV. Do not use newer std APIs than the MSRV allows.
- Do not add `#![allow(...)]` or `#[allow(...)]` unless it is needed. Add the narrowest scope, and put a comment that says why. Prefer `#[expect(...)]` when the project's MSRV allows it, because it warns when it becomes useless.
- Prefer configuring lints in `Cargo.toml` under `[lints]` instead of scattering attributes.
- Libraries SHOULD add `#![forbid(unsafe_code)]` when they contain no `unsafe`.
- Libraries SHOULD enable `missing_docs` (warn or deny).

---

## 2. Formatting (Official Style)

`cargo fmt` with default settings is the final judge. In short:

- 4 spaces for indentation. Never tabs.
- Maximum line width: 100 characters.
- Block indent, not visual indent.
- Trailing commas in multi-line lists, arguments, struct literals, and match arms.
- `//` for comments, with one space after it. Avoid `/* */`.
- `{` on the same line as the declaration. Empty blocks are `{}`.
- At most one blank line between items.
- Do not fight the formatter. Do not add `#[rustfmt::skip]` unless the layout really matters (for example, a table of numbers).

---

## 3. Naming

| Item                                           | Convention                                  | Example                     |
| ---------------------------------------------- | ------------------------------------------- | --------------------------- |
| Variables, functions, methods, modules, fields | `snake_case`                                | `user_id`, `parse_config`   |
| Types, traits, enum variants                   | `UpperCamelCase`                            | `UserProfile`, `HttpClient` |
| Constants and statics                          | `SCREAMING_SNAKE_CASE`                      | `MAX_RETRIES`               |
| Lifetimes                                      | short lowercase                             | `'a`, `'de`, `'src`         |
| Type parameters                                | short `UpperCamelCase`                      | `T`, `Key`, `Item`          |
| Macros                                         | `snake_case!`                               | `my_macro!`                 |
| Crates, feature names                          | `kebab-case` or `snake_case`, be consistent | `my-crate`                  |

Extra rules:

- Treat acronyms as words: `Uuid`, `HttpClient`, `XmlReader`. Not `UUID` or `HTTPClient`.
- No `get_` prefix for simple getters: use `name()`, not `get_name()`. (A method called `get` is fine for lookup by key or index, like `HashMap::get`.)
- Conversion names show the cost and ownership:
  - `as_*`: cheap, borrowed to borrowed.
  - `to_*`: costly, makes a new value (or cheap for `Copy` types).
  - `into_*`: takes `self` (consumes it).
- Getters for a field named `x` are `x()` and `x_mut()`.
- Iterator methods: `iter()`, `iter_mut()`, `into_iter()`.
- Boolean methods read like a question: `is_empty()`, `has_children()`, `can_retry()`.
- Avoid filler words (`Manager`, `Helper`, `Util`, `Data`) and repeated module names (`user::UserError` → `user::Error`).
- Short scope → short name. Large scope → descriptive name.

---

## 4. Ownership, Borrowing, and Types

- Prefer borrowing over cloning.
- In parameters, prefer `&str` over `&String`, `&[T]` over `&Vec<T>`, `&T` over `&Box<T>`, and `&Path` over `&PathBuf`.
- Use `impl AsRef<str>`, `impl Into<String>`, or `impl AsRef<Path>` only when it makes the call site clearly nicer. Do not use them everywhere.
- Do not call `.clone()` just to silence the borrow checker. First think about the design: can you borrow, reorder, or restructure?
- Avoid `Rc`, `Arc`, `RefCell`, `Mutex` unless the design truly needs shared ownership or shared mutation.
- Prefer concrete types, then generics, then `dyn Trait`. Use `dyn Trait` when you need runtime choice or want to cut compile time and code size.
- Use newtypes to protect rules (for example, `struct UserId(u64);`).
- Use enums instead of booleans or strings for states. Avoid functions with several `bool` parameters.
- Use `Option` for "maybe missing" and never use sentinel values like `-1` or `""`.
- Use `#[must_use]` on functions and types where ignoring the result is likely a bug.
- Use `Cow<'_, str>` when a function sometimes needs to allocate and sometimes not.
- Prefer `u32`/`usize` etc. that match the real range. Avoid `as` casts that can lose data. Use `TryFrom` / `try_into()`, or `From` when it cannot fail.

### Traits to implement on public types

Implement these when they make sense: `Debug` (always), `Clone`, `PartialEq`, `Eq`, `Hash`, `PartialOrd`, `Ord`, `Default`, `Display` (for user-facing text), `From`/`TryFrom`. For public types, also think about `Send` and `Sync`, and test that they hold if users need them.

---

## 5. Error Handling

- Use `Result<T, E>` for errors that can happen in normal use.
- Use panics only for bugs (broken invariants) or when recovery is impossible.
- Use `?` to pass errors up. Add context where it helps.

**Libraries**

- Use a real error type: a struct or an enum. `thiserror` is a good, common choice.
- Do not use `String`, `&str`, or `Box<dyn Error>` as the main public error type.
- Consider `#[non_exhaustive]` on public error enums so adding variants is not a breaking change.
- Implement `std::error::Error`, and return the underlying cause from `source()`.
- Error messages: lowercase start, no final period (`"failed to open config"`).

**Applications (binaries)**

- `anyhow` (or `eyre`) with `.context("...")` is fine and often best.
- Print errors for humans at the top level (`main`), and use a proper exit code.

**`unwrap` and `expect`**

- MUST NOT appear in library code or production paths.
- Allowed in tests, examples, and benchmarks.
- Allowed where it is provably impossible to fail. Then use `.expect("why this cannot fail")` and make the message explain the reason.
- Also watch for hidden panics: `slice[i]` indexing, `a / b` with `b == 0`, integer overflow in release builds, `RefCell` double borrow, and `String` slicing at non-char boundaries. Prefer `.get(i)`, `checked_*`, `saturating_*`, or `wrapping_*` where inputs are not trusted.

**Docs for errors**

- `# Errors` section: when and why the function returns `Err`.
- `# Panics` section: when it can panic.
- `# Safety` section: what the caller must guarantee (for `unsafe fn`).

---

## 6. Documentation

- Every public item MUST have a `///` doc comment. Start with one short summary line.
- Add crate-level docs with `//!` that say what the crate is for, with a short example.
- Public functions SHOULD have a runnable example. Examples use `?`, not `unwrap()`:

  ````rust
  /// Parses a port number.
  ///
  /// # Errors
  ///
  /// Returns [`ParsePortError`] if `input` is not a number from 1 to 65535.
  ///
  /// # Examples
  ///
  /// ```
  /// # use my_crate::parse_port;
  /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
  /// let port = parse_port("8080")?;
  /// assert_eq!(port, 8080);
  /// # Ok(())
  /// # }
  /// ```
  pub fn parse_port(input: &str) -> Result<u16, ParsePortError> {
      // ...
  }
  ````

- Link related items with ``[`Type`]``.
- Explain **why**, not only **what**. Do not write comments that repeat the code.
- Keep comments true. When you change code, update its comments.
- Use `TODO(name):` or `FIXME(name):` for later work. Do not leave unexplained TODOs.

---

## 7. API Design

Follow the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/) checklist. In particular:

- `new()` is the main constructor. Use `with_*` for variants, and `from_*` for conversions from a specific source.
- If a type has many optional settings, use a builder or a config struct with `Default`.
- Prefer `From`, `TryFrom`, `FromStr`, and `AsRef` over custom conversion functions.
- Keep the public API small. Use `pub(crate)` or `pub(super)` by default, and `pub` only on purpose.
- Do not expose private details in public signatures (for example, types from a dependency) unless you accept that the dependency is part of your API.
- Use `#[non_exhaustive]` on public enums and structs that may grow.
- Use sealed traits if outside users must not implement your trait.
- Do not force users into `Rc`, `RefCell`, `Mutex`, or lifetimes that are hard to use.
- Functions should take what they need, and return what the caller will want. Return owned values when the caller would have to clone anyway.
- Return `impl Iterator<Item = T>` (or a named iterator type) instead of collecting into a `Vec` when the caller may not need all items.
- Think about semver: removing or changing public items is a breaking change.

---

## 8. Control Flow and Idioms

- Prefer `match` on enums. Be exhaustive. Avoid `_ =>` unless it is truly right for all future variants (and for foreign `#[non_exhaustive]` types, `_` is required, so use it there).
- Use `if let`, `while let`, and `let ... else` to reduce nesting:

  ```rust
  let Some(user) = users.get(&id) else {
      return Err(Error::UserNotFound(id));
  };
  ```

- Use early returns and `?` to keep the happy path flat.
- Prefer iterators (`map`, `filter`, `fold`, `collect`) when they are clearer than a loop. Use a plain `for` loop when it is clearer or has side effects.
- Use `Vec::with_capacity`, `String::with_capacity`, and `extend` when the size is known.
- Do not collect into a `Vec` only to iterate again.
- Prefer `Option` and `Result` combinators (`map`, `and_then`, `ok_or`, `unwrap_or_default`) over manual `match` when they read well. Do not chain so many that it gets hard to read.
- Use `#[derive(...)]` instead of writing trait impls by hand when the derived behavior is right.
- Use `Self` inside `impl` blocks.
- Use `matches!` for simple pattern checks.
- Prefer `const` and `static` over magic numbers. Name your constants.
- Do not use `mem::transmute`, `mem::forget`, or `Box::leak` unless truly needed, and then explain why.

---

## 9. Concurrency and Async

- Do not hold a `Mutex` or `RwLock` guard across an `.await`. Copy out what you need and drop the guard first. (If you must lock across an await, use an async-aware lock and explain why.)
- Do not block in async code (no `std::thread::sleep`, no heavy CPU work, no blocking I/O). Use async versions or `spawn_blocking`.
- Do not use `block_on` inside async code.
- Prefer message passing (channels) or ownership transfer over shared state when it is simple to do so.
- Make futures `Send` when they will run on a multi-threaded runtime. Do not hold non-`Send` values across an `.await`.
- Prefer structured concurrency (`JoinSet`, `join!`, `select!`) over detached `spawn` calls. If you detach a task, handle its errors and think about shutdown.
- Make async code cancel-safe where it matters, and say in the docs when it is not.
- Use atomics only when you understand the memory ordering. Default to `SeqCst` if unsure, and explain weaker orderings in a comment.
- Do not use `unwrap()` on lock results in production code without thinking about poisoning. Handle it, or use a lock type that does not poison.

---

## 10. Unsafe Code

- Avoid `unsafe`. First look for a safe solution or a well-tested crate.
- Keep each `unsafe` block as small as possible.
- Every `unsafe` block MUST have a `// SAFETY:` comment that says why it is sound.
- Every `unsafe fn` and `unsafe trait` MUST have a `# Safety` doc section that lists what callers must guarantee.
- Wrap `unsafe` inside a safe API so that callers cannot cause undefined behavior.
- Never hide or "hope away" undefined behavior. When in doubt, do not use `unsafe`.
- Test `unsafe` code with Miri when possible (`cargo +nightly miri test`).
- Enable `unsafe_op_in_unsafe_fn` (default in edition 2024) and keep `unsafe` blocks explicit inside `unsafe fn`.

---

## 11. Security and Robustness

- Treat all outside input as untrusted: files, network data, environment variables, command-line arguments.
- Never put secrets (passwords, API keys, tokens) in code, logs, or error messages.
- Check sizes and limits before allocating based on input.
- Use checked arithmetic when values come from outside.
- Do not build shell commands, SQL, or file paths by joining raw strings. Use parameterized queries, `Command::arg`, and `Path`/`PathBuf` APIs, and guard against `..` path traversal.
- Use well-known crates for crypto. Never write your own.
- Keep dependencies few and well-maintained. Consider `cargo audit` and `cargo deny` in CI.

---

## 11.1 Random Number Generation & Bitwise Arithmetic

- **Never use naive PRNGs for security:** Linear generators (such as `xorshift`, `pcg`, or LCGs) MUST NOT be used for keys, tokens, nonces, or security-sensitive identifiers. Use `rand::rngs::OsRng` or dedicated cryptographic crates.
- **Fail fast on invalid PRNG seeds:** Deterministic generators with prohibited zero states (e.g., XorShift) MUST NOT silently fall back to a hardcoded constant seed. Return `Result::Err` or panic on invalid input to prevent independent components from silently generating identical sequences.
- **Extract high-order bits for booleans and small integers:** Linear feedback and shift-register generators exhibit poor diffusion and strong linear dependencies in lower bits. When producing booleans or flags, inspect the most significant bits (`(x as i64) < 0` or `x >> 63 == 1`), never the lowest bit (`x & 1`).
- **Define range boundaries explicitly:**
  - Half-open range `[min, max)`: Explicitly check that `min < max`. Never return `min` when `min == max` (an empty range has zero elements).
  - Inclusive range `[min, max]`: Compute the span using checked arithmetic: `(max - min).checked_add(1)`.
- **Prevent target-architecture truncation:** Always compute range modulo and bitwise bounds using the wide integer type (`u64`) _before_ casting to pointer-sized types (`usize`) to prevent 32-bit truncation on targets like `wasm32` or ARM Cortex.
- **Document modulo bias:** State in doc comments when a range helper accepts `% diff` modulo bias for performance. Use rejection sampling (e.g., Lemire's reduction) when uniform distribution is strictly required.

---

## 12. Logging and Observability

- Libraries SHOULD NOT print to stdout or stderr. Use the `log` or `tracing` facade and let the application choose the output.
- Use clear levels: `error`, `warn`, `info`, `debug`, `trace`.
- Log structured fields when using `tracing`. Do not log secrets or large payloads.
- Use `println!`/`eprintln!` only in binaries, examples, and tests.

---

## 13. Performance

- Write clear code first. Measure before you optimize.
- Do not add `unsafe` or clever tricks for speed without a benchmark that shows the need.
- Avoid needless allocation: reuse buffers, borrow, and use iterators.
- Prefer `&[T]` and `&str` in hot paths.
- Choose data structures on purpose (`Vec` vs `VecDeque` vs `HashMap` vs `BTreeMap`).
- Use `criterion` (or the project's tool) for benchmarks.

---

## 14. Modules and Project Structure

- Put `use` statements at the top, after inner attributes and module docs. Do not put `use` in the middle of functions unless it keeps the scope tight on purpose.
- Prefer explicit imports. Avoid `use foo::*;` except for preludes and `use super::*;` in test modules.
- Declare `mod` items before other items.
- Keep modules small and focused. Split files that grow too large.
- Use `pub use` to build a clean public API surface (re-exports).
- Keep `main.rs` thin: put logic in a library crate (`lib.rs`) so it is easy to test.
- Use Cargo features to switch optional parts on and off. Features MUST be additive.

---

## 15. Testing

- Write unit tests for non-trivial logic in a `#[cfg(test)] mod tests`.
- Put integration tests in `tests/`.
- Test error cases and edge cases, not only the happy path.
- Test names should say what they check: `parse_port_rejects_zero`.
- Prefer `assert!`, `assert_eq!`, and `assert_matches!`-style checks. In tests that can fail with an error, return `Result<(), E>` and use `?`.
- `unwrap()` and `expect()` are acceptable in tests. Prefer a clear assertion message when one helps.
- Tests MUST be deterministic and independent. No sleeping for timing, no shared global state, no network access unless marked and isolated.
- Consider property tests (`proptest`) and fuzzing for parsers and other input-heavy code.
- Doc examples MUST compile and pass (`cargo test --doc`).

---

## 16. Forbidden Patterns

AI agents MUST NOT generate:

- Code that fails `cargo fmt` or has Clippy warnings (unless the user explicitly allows it).
- `.unwrap()` or `.expect()` in library or production code, unless the panic is impossible and explained.
- `String` or `Box<dyn Error>` as a library's main public error type.
- Tabs, or visual indentation.
- `get_` prefixes on plain getters.
- Needless `.clone()`, `.to_string()`, or allocations.
- Public APIs that force `Rc`, `RefCell`, or `Mutex` on users without a clear reason.
- `unsafe` blocks without a `// SAFETY:` comment.
- Wildcard matches that hide new variants when each new variant needs its own handling.
- Made-up crate names, functions, or features.
- Holding a lock guard across `.await`.
- Secrets or credentials in source code.
- `todo!()`, `unimplemented!()`, or placeholder code left in finished work (unless the user asked for a skeleton).
- `#[allow(...)]` used only to hide a warning you did not fix.
- Silent fallback seeds (e.g., `if seed == 0 { seed = DEFAULT; }`) in PRNG constructors that conceal uninitialized or invalid states.
- Extracting boolean values or sub-ranges from bit 0 (`& 1`) of linear shift/XorShift generators.
- Range sampling functions that return `min` on empty half-open ranges where `min >= max`.
- Downcasting 64-bit random words to `usize` before applying modulo arithmetic.

---

## 17. How AI Agents Should Respond

When you give Rust code:

1. Give complete code that compiles, with the needed `use` lines. If you cannot run it, say that you have not compiled it.
2. Show the most idiomatic solution first.
3. If more than one good approach exists, pick the one that best follows these rules, and explain in one or two sentences why.
4. When editing existing code, keep its style, unless the style breaks a MUST rule. If you break a SHOULD rule to stay consistent, that is fine.
5. If a rule in this file clashes with what the user asked, follow the user and mention the clash briefly.
6. If the task is unclear, or has a risky choice (for example, adding `unsafe`, a new dependency, or a breaking API change), ask or state your assumption clearly.
7. When you are reviewing code, report problems in order of importance: bugs and safety first, then API and design, then style.
8. Be honest about limits: say what you did not test, and what you are not sure about.

---

## Quick Checklist

Before you finish, check:

- [ ] `cargo fmt --check` and `cargo fmt --check -- --config newline_style=Unix` pass (CI runs on Linux; the second command catches line-ending and trailing-blank-line problems that Windows hides)
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo test` and `cargo test --doc` pass
- [ ] Names follow the conventions (acronyms as words, no `get_`)
- [ ] Borrowing is preferred over cloning
- [ ] Errors use `Result`, with proper types, and no stray `unwrap`
- [ ] No hidden panics from indexing, division, or overflow on untrusted input
- [ ] Public items have docs, and `# Errors` / `# Panics` / `# Safety` where needed
- [ ] Matches are exhaustive where it matters
- [ ] Every `unsafe` has a `// SAFETY:` comment and the smallest scope
- [ ] Async code does not block or hold locks across `.await`
- [ ] No new dependency, or the new one is justified
- [ ] Public API follows the Rust API Guidelines and is as small as possible
- [ ] Changes are small and do not touch unrelated code
- [ ] PRNGs reject invalid seeds explicitly instead of silently substituting a shared default
- [ ] Bitwise extractions and booleans sample from high-order bits, not bit 0
- [ ] Range bounds logic handles empty intervals correctly without silent out-of-spec returns
- [ ] Arithmetic operations on wide integers (`u64`) are completed before casting to `usize`

---

## Project Roadmap

The plan for this project is in `cachedb-roadmap.md`.
Read it before starting any big task.
Do not build things that are not in the roadmap without asking me.

**End of Rules**

These rules apply to all Rust code written by AI agents. The user can override a specific rule by saying so clearly. When in doubt, follow the official Rust Style Guide and the Rust API Guidelines.
