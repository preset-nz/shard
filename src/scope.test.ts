import { describe, expect, it } from 'vitest';
import { ARRANGEMENT_NODES, kindOfNode, NODES, nodeById, setNodeWords } from '@/scope';

describe('node words', () => {
  it('names the arrangement nodes by their kinds', () => {
    expect(kindOfNode('arrangement.filter')).toBe('filter');
    expect(kindOfNode('arrangement.amp')).toBe('master');
    expect(kindOfNode('grain')).toBe('grain');
  });

  it('reads a label and summary from the words Rust gave', () => {
    setNodeWords([
      { id: 'grain', label: 'Granular', summary: 'A cloud.' },
      { id: 'master', label: 'Output', summary: 'The master.' },
    ]);
    expect(nodeById('grain')?.label).toBe('Granular');
    expect(nodeById('grain')?.summary).toBe('A cloud.');
    expect(nodeById('arrangement.amp')?.label).toBe('Output');
  });

  it('holds no label of its own', () => {
    setNodeWords([]);
    for (const n of [...NODES, ...ARRANGEMENT_NODES]) expect(n.label).toBe('');
  });
});
