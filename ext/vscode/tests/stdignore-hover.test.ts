import { describe, expect, test } from 'bun:test';
import { renderHoverMarkdown } from '../src/stdignore/hover-render';

describe('renderHoverMarkdown', () => {
  test('reports no matches when total_count is zero', () => {
    const text = renderHoverMarkdown({
      pattern: 'target/',
      matches: [],
      total_count: 0,
      truncated: false,
      walk_truncated: false,
    });
    expect(text).toContain('no matches in workspace');
    expect(text).toContain('target/');
  });

  test('lists matches with count when below limit', () => {
    const text = renderHoverMarkdown({
      pattern: '*.log',
      matches: ['app.log', 'errors.log'],
      total_count: 2,
      truncated: false,
      walk_truncated: false,
    });
    expect(text).toContain('2 matches');
    expect(text).toContain('app.log');
    expect(text).toContain('errors.log');
  });

  test('surfaces "showing N of M" when truncated', () => {
    const text = renderHoverMarkdown({
      pattern: '*.log',
      matches: ['a.log', 'b.log', 'c.log'],
      total_count: 100,
      truncated: true,
      walk_truncated: false,
    });
    expect(text).toContain('showing 3 of 100');
  });

  test('appends walk-truncated note when applicable', () => {
    const text = renderHoverMarkdown({
      pattern: '**',
      matches: ['a', 'b'],
      total_count: 50_000,
      truncated: true,
      walk_truncated: true,
    });
    expect(text).toContain('Walk capped');
  });

  test('escapes markdown special characters in path entries', () => {
    const text = renderHoverMarkdown({
      pattern: 'foo_*_bar',
      matches: ['some/path_with_underscores.rs'],
      total_count: 1,
      truncated: false,
      walk_truncated: false,
    });
    expect(text).toContain('path\\_with\\_underscores');
  });

  test('singular vs plural label respects total_count = 1', () => {
    const text = renderHoverMarkdown({
      pattern: 'Cargo.lock',
      matches: ['Cargo.lock'],
      total_count: 1,
      truncated: false,
      walk_truncated: false,
    });
    expect(text).toContain('1 match');
    expect(text).not.toContain('1 matches');
  });
});
