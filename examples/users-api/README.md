# users-api — the "effort/payoff ratio" demo

📖 English · [Français](README.fr.md)

A realistic mini project : a `users` REST API with **5 endpoints**, **Rust** server side + **TypeScript** client side.

The point of this demo : show **two approaches** to documenting the 10 functions, and prove that the "smart" version takes **~57 lines of template** to produce the same page that the "verbose" version takes ~170 lines for — and the smart one stays at 57 lines no matter how many endpoints you add.

## Layout

```
users-api/
├── README.md                       ← you are here
├── COMPARISON.md                   ← read this one first
├── src/
│   ├── server.rs                   ← 5 Rust endpoints annotated `@doc users.X`
│   └── client.ts                   ← 5 TS wrappers annotated `@doc client.users.X`
├── docs-src/                       ← `.md` templates (sources, what YOU write)
│   ├── 01-api-verbose.md           ← explicit template : ~170 lines
│   └── 02-api-smart.md             ← smart template : ~57 lines
└── docs-rendered/                  ← rendered markdown (what end users see)
    ├── 01-api-verbose.md           ← render of 01 (identical to 02)
    └── 02-api-smart.md             ← render of 02 (identical to 01)
```

## Run it for real

From the root of the `standardoc` repo :

```sh
cargo run -p standardoc -- transform examples/users-api examples/users-api/docs-src/02-api-smart.md
```

You get on stdout exactly the content of `docs-rendered/02-api-smart.md`. Change a type in `src/server.rs`, re-run the command, the render reflects the change automatically. **No `.md` to touch.**

## The punchline

- Add a 6th function in `server.rs` ? You write the function + its `@doc`. You touch **zero templates**. The web/markdown page updates on the next build.
- With the verbose version, you have to hand-edit the template to add a 6th section. Then for the 7th. Then for the 8th. That's exactly the trap of TypeDoc / Docusaurus in manual mode.

Open `COMPARISON.md` to see the two templates side by side.
