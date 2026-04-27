---
title: Welcome
order: 0
---

# Welcome

This is your Standardoc site. Edit this file at `.standardoc/pages/index.md`, or click **Edit** in the top-right corner to update it directly from the web UI.

## Auto-generated reference

Standardoc has scanned your codebase and made every documentable symbol available under the **Reference** section in the sidebar. Click any entry to see its signature, parameters, and `@doc` annotations.

## Adding pages

Drop new `.md` or `.mdx` files into `.standardoc/pages/` — they show up automatically. Use the `NN-` prefix on filenames to control ordering (e.g. `01-getting-started.md`).

## Live values

Anywhere in a page you can inject a live value from the index using the DSL — for example `{{ @doc.SomeKey:label }}` or `{{ @doc.SomeKey:description }}`. The value updates automatically when the source code changes.
