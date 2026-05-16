declare module '*.module.scss' {
	const styles: Readonly<Record<string, string | undefined>>;
	export default styles;
}

declare module '*.scss' {
	const css: string;
	export default css;
}
