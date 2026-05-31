// Ambient SCSS module declarations. The viz lib (`@standarx/standardoc-viz`)
// is consumed as source via tsconfig `paths`, so its components'
// `import s from './x.module.scss'` lines are typechecked in this
// project too. The bundler turns these into runtime <style> injectors
// (default-exporting the classname map / ''); this declaration gives
// tsc the matching shape so the webview typecheck stays clean.
declare module '*.module.scss' {
  const classes: Record<string, string>;
  export default classes;
}

declare module '*.scss' {
  const css: string;
  export default css;
}
