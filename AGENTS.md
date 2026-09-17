Always use the Makefile. The main verification targets are:

```sh
make check       # rustfmt check, Clippy with warnings denied, Rust tests
make build       # optimized release binary

```
WARNING: NEVER RUN `cargo fmt` DIRECTLY
ALWAYS RUN:
    make fmt

---

- Optimize only after you know what actually needs optimizing. 
- Adding a dependency should be treated as taking ownership of its consequences, You must consult first and you must justify before adding one dependency.
- Complexity is a liability. Every abstraction, dependency, and optimization has to earn its place.

---
