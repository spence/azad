# Style Guide

## Code Formatting

- Run `cargo fmt` to format the code.
- Use `.rustfmt.toml` to configure the formatter.
- Max line length is 100 characters.
- Use 2 spaces for indentation.

## Import Ordering

1. `mod` declarations first
2. `pub use` re-exports next
3. `std::*` (standard library)
4. External crates
5. `self::*`, `super::*`, `crate::*` (sorted together)

- `use` statements never span multiple lines (wrap at 99 characters)
- groups are separated by blank lines

## Code Ordering

1. Constants
2. Static variables
3. Primary objects or functionality (keep impls together)
4. Remaining objects by importance
5. Helpers
6. Tests

## Commenting Rules

1. **Minimalism**

- Generally, only comment when necessary.
- Most code has no comments at all.
- Keep comments brief.

2. **Explain WHY, not WHAT**

- Explain why (constraints, invariants, or reasoning) over what (the code does).
- Clarify purpose when not obvious.

3. **No comments**:

- Boilerplate and obvious code.
- Self-explanatory function signatures
- Simple getters/setters
- Standard trait implementations (e.g., Debug, Display, etc.)
- Test code
- Absolutely no separator comments.

4. **Formatting**:

- Use backticks for code references** (e.g., `` [`ErasedEntry`] ``)
- Field comments go above the field**, not inline.
- Single-line preferred

5. **SAFETY comments are mandatory for `unsafe` blocks**

- Except for unsafe impl Sync/Send.

```rust
// SAFETY: Matching TypeId guarantees that Box<V> is Box<A>
// (std::marker::Unsize<V> would safely cast it, but it's nightly only)
let entry = unsafe { ... };
```

6. **Library crates: doc all public types and non-obvious methods**

- Provide `///` on public structs, traits, macros, and any method whose behavior or constraints aren't fully captured by the signature.
- Write file comments on the main lib.rs.
- Provide doc comment examples for the primary public objects.
- Skip simple getters and self-explanatory signatures.

## Commit Convention

[Conventional Commits v1.0.0](https://www.conventionalcommits.org/en/v1.0.0/)

```
<type>[optional scope]: <description>

[optional body]

[optional footer(s)]
```

### Specification

1. Commits MUST be prefixed with a type, which consists of a noun, `feat`, `fix`, etc., followed by the OPTIONAL scope, OPTIONAL `!`, and REQUIRED terminal colon and space.
2. The type `feat` MUST be used when a commit adds a new feature to your application or library.
3. The type `fix` MUST be used when a commit represents a bug fix for your application.
4. A scope MAY be provided after a type. A scope MUST consist of a noun describing a section of the codebase surrounded by parenthesis, e.g., `fix(parser):`.
5. A description MUST immediately follow the colon and space after the type/scope prefix.
6. A longer commit body MAY be provided after the short description, providing additional contextual information about the code changes. The body MUST begin one blank line after the description.
7. A commit body is free-form and MAY consist of any number of newline separated paragraphs.
8. One or more footers MAY be provided one blank line after the body. Each footer MUST consist of a word token, followed by either a `:<space>` or `<space>#` separator, followed by a string value.
9. A footer's token MUST use `-` in place of whitespace characters, e.g., `Acked-by`.
10. A footer's value MAY contain spaces and newlines, and parsing MUST terminate when the next valid footer token/separator pair is observed.
11. Breaking changes MUST be indicated in the type/scope prefix of a commit, or as an entry in the footer.
12. If included as a footer, a breaking change MUST consist of the uppercase text `BREAKING CHANGE`, followed by a colon, space, and description.
13. If included in the type/scope prefix, breaking changes MUST be indicated by a `!` immediately before the `:`.
14. Types other than `feat` and `fix` MAY be used in your commit messages, e.g., `docs: update ref docs.`
15. The units of information that make up Conventional Commits MUST NOT be treated as case-sensitive by implementors, with the exception of `BREAKING CHANGE` which MUST be uppercase.
16. `BREAKING-CHANGE` MUST be synonymous with `BREAKING CHANGE`, when used as a token in a footer.

### Types

| Type       | Description                                           |
| ---------- | ----------------------------------------------------- |
| `feat`     | A new feature                                         |
| `fix`      | A bug fix                                             |
| `docs`     | Documentation only changes                            |
| `style`    | Changes that do not affect the meaning of the code    |
| `refactor` | A code change that neither fixes a bug nor adds a feature |
| `perf`     | A code change that improves performance               |
| `test`     | Adding missing tests or correcting existing tests     |
| `build`    | Changes that affect the build system or dependencies  |
| `ci`       | Changes to CI configuration files and scripts         |
| `chore`    | Other changes that don't modify src or test files     |


### Examples

```
fix(api): handle null response from upstream service
```

```
feat(auth)!: replace session tokens with JWTs

BREAKING CHANGE: session-based auth is no longer supported.
```
