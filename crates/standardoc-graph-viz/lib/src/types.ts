export type RenderMode = 'canvas2d' | 'webgpu';

export type StatusKind = 'booting' | 'ready' | 'loading' | 'error';

export interface Status {
  readonly kind: StatusKind;
  readonly text: string;
}
