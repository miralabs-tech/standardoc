import { describe, expect, test } from 'bun:test';
import { stripJsonc } from '../../src/init/jsonc';

describe('stripJsonc', () => {
  test('leaves plain JSON untouched', () => {
    const json = '{"a": 1, "b": [2, 3]}';
    expect(JSON.parse(stripJsonc(json))).toEqual({ a: 1, b: [2, 3] });
  });

  test('strips line comments', () => {
    const src = '{\n  "a": 1 // trailing\n  // standalone\n}';
    expect(JSON.parse(stripJsonc(src))).toEqual({ a: 1 });
  });

  test('strips block comments', () => {
    const src = '{ /* x */ "a": /* y */ 1 }';
    expect(JSON.parse(stripJsonc(src))).toEqual({ a: 1 });
  });

  test('strips trailing commas in objects and arrays', () => {
    const src = '{ "a": [1, 2,], "b": 3, }';
    expect(JSON.parse(stripJsonc(src))).toEqual({ a: [1, 2], b: 3 });
  });

  test('preserves comment-like and comma-like sequences inside strings', () => {
    const src = '{ "url": "http://x//y", "tpl": "/* not a comment */", "arr": "a,]b" }';
    expect(JSON.parse(stripJsonc(src))).toEqual({
      url: 'http://x//y',
      tpl: '/* not a comment */',
      arr: 'a,]b',
    });
  });

  test('handles escaped quotes inside strings', () => {
    const src = '{ "q": "a\\"// b" }';
    expect(JSON.parse(stripJsonc(src))).toEqual({ q: 'a"// b' });
  });
});
