import type { RenderMode } from '../../types';

export interface ToolbarModeRequestDetail {
	readonly mode: RenderMode;
}

export interface ToolbarFlagChangeDetail {
	readonly value: boolean;
}
