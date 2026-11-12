import { describe, expect, it } from 'vitest';
import { modelFromPick, OTHER, serverFromPick, serverShown } from './modelPick';

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

describe('server pick', () => {
  const known = ['http://localhost:8000/v1', 'http://localhost:1234/v1'];
  it('ignores null and the pick already showing', () => {
    expect(serverFromPick(null, known[0])).toBeNull();
    expect(serverFromPick(known[0], known[0])).toBeNull();
    expect(serverFromPick(OTHER, OTHER)).toBeNull();
  });
  it('saves a known server and only reveals the input for Other', () => {
    expect(serverFromPick(known[1], known[0])).toEqual({ save: known[1] });
    expect(serverFromPick(OTHER, known[0])).toEqual({ other: true });
  });
  it('selects Other for a saved URL no known server has', () => {
    expect(serverShown(known[1], known)).toBe(known[1]);
    expect(serverShown('http://10.0.0.5:9000/v1', known)).toBe(OTHER);
  });
});
