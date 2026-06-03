// Text imports (`import x from './foo.md' with { type: 'text' }`) resolve
// to the file's contents as a string. Bun handles this at runtime/bundle;
// this ambient declaration is what lets `tsc` type the import without
// resolving the path on disk (the shared asset lives outside `src/`).
declare module '*.md' {
  const content: string;
  export default content;
}
