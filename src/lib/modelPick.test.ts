import { describe, expect, it } from 'vitest';
import { modelFromPick } from './modelPick';

describe('modelFromPick', () => {
  it('ignores a null or undefined pick, never saving "null"', () => {
    expect(modelFromPick(null, 'qwen/qwen3.6-35b-a3b')).toBeNull();
    expect(modelFromPick(undefined, '')).toBeNull();
  });
  it('maps "auto" to empty, and ignores a pick of what is saved', () => {
    expect(modelFromPick('auto', 'some/model')).toBe('');
    expect(modelFromPick('auto', '')).toBeNull();
    expect(modelFromPick('some/model', 'some/model')).toBeNull();
  });
  it('returns a new model id', () => {
    expect(modelFromPick('other/model', 'some/model')).toBe('other/model');
  });
});
