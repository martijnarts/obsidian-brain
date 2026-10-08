import { describe, it, expect } from 'vitest';
import { compileUserRegex } from '../../src/util/safe-regex.js';

describe('compileUserRegex', () => {
  it('escapes metacharacters in a literal pattern', () => {
    const re = compileUserRegex('a.b (c)', '', true);
    expect(re.test('xa.b (c)y')).toBe(true);
    expect(re.test('aXb (c)')).toBe(false);
    expect(compileUserRegex('(a+)+', 'g', true).test('x(a+)+y')).toBe(true);
  });

  it('passes the flags through', () => {
    expect(compileUserRegex('Foo', 'i', true).test('foo')).toBe(true);
    expect(compileUserRegex('Foo', '', true).test('foo')).toBe(false);
    expect(compileUserRegex('a', 'gm').flags).toBe('gm');
  });

  it('refuses patterns over 500 characters, literal or not', () => {
    expect(() => compileUserRegex('a'.repeat(501), 'g')).toThrow(/too long \(501 chars, max 500\)/);
    expect(() => compileUserRegex('a'.repeat(501), 'g', true)).toThrow(/too long/);
    expect(compileUserRegex('a'.repeat(500), '')).toBeInstanceOf(RegExp);
  });

  it('refuses quantified groups that contain a quantifier', () => {
    for (const p of ['(a+)+', '(a+)+$', '(\\w*x)*', '(a{1,})+', '(a|b*){2,}', '(a+){3}']) {
      expect(() => compileUserRegex(p, 'g'), p).toThrow(/quantified group/);
    }
  });

  it('refuses invalid syntax', () => {
    expect(() => compileUserRegex('(', 'g')).toThrow(/Invalid regex/);
    expect(() => compileUserRegex('[', 'g')).toThrow(/Invalid regex/);
  });

  it('accepts ordinary groups and escaped parentheses', () => {
    expect(compileUserRegex('(a+)', 'g').test('aa')).toBe(true);
    expect(compileUserRegex('(foo|bar)+', '').test('barfoo')).toBe(true);
    expect(compileUserRegex('\\(a+\\)+', '').test('(aa))')).toBe(true);
  });
});
