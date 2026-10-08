import { errorMessage } from './errors.js';

const MAX_PATTERN_LENGTH = 500;

/**
 * A group that holds a quantifier and is itself quantified, like `(a+)+`,
 * `(\w*x)*` or `(a{1,})+`. Such patterns backtrack exponentially on a
 * near-miss, and a JS regex cannot be interrupted mid-match, so they are
 * refused up front.
 */
const NESTED_QUANTIFIER = /\((?:[^()\\]|\\.)*(?:[+*]|\{\d+,?\d*\})(?:[^()\\]|\\.)*\)(?:[+*]|\{\d+,?\d*\})/;

/**
 * Compile a client regex with the guards every pattern tool applies: a
 * length cap, and no quantified group that itself holds a quantifier.
 * `literal` escapes the pattern so it matches as plain text.
 */
export function compileUserRegex(pattern: string, flags: string, literal = false): RegExp {
  if (pattern.length > MAX_PATTERN_LENGTH) {
    throw new Error(`Pattern is too long (${pattern.length} chars, max ${MAX_PATTERN_LENGTH}).`);
  }
  if (literal) return new RegExp(pattern.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), flags);
  if (NESTED_QUANTIFIER.test(pattern)) {
    throw new Error('Pattern has a quantified group that contains a quantifier, like `(a+)+`. Rewrite it without the nesting.');
  }
  try {
    return new RegExp(pattern, flags);
  } catch (err) {
    throw new Error(`Invalid regex: ${errorMessage(err)}`);
  }
}
