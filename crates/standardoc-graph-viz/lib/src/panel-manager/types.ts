// Discriminated union of panel kinds the shell can spawn. Every new
// kind adds an entry here AND in `PanelPropsMap` so the manager API
// stays type-safe end-to-end (open(kind, props) refuses mismatched
// shapes at the call site).

export type PanelKind = 'compare';

export interface PanelPropsMap {
	readonly compare: {
		readonly leftFqdn: string;
		readonly rightFqdn: string;
	};
}

export interface PanelInstance<K extends PanelKind = PanelKind> {
	readonly id: string;
	readonly kind: K;
	readonly props: PanelPropsMap[K];
	readonly title: string;
}

export interface PanelManagerState {
	readonly panels: ReadonlyArray<PanelInstance>;
	readonly activeId: string | null;
}

export interface PanelChangeDetail extends PanelManagerState {
	readonly changedId: string | null;
}
