import { describe, expect, it } from 'vitest';
import type { AgentEvent } from '@/lib/agent';
import { emptyConversation, isStreaming, reduceChat, type Turn, transcript } from './agentChat';

const run = (events: AgentEvent[]) => events.reduce(reduceChat, emptyConversation);
const start: AgentEvent = { kind: 'start', prompt: 'a beat', model: 'qwen' };
const assistant = (turns: Turn[]) => {
  const t = turns[turns.length - 1];
  if (t?.from !== 'assistant') throw new Error('no assistant turn');
  return t;
};

describe('reduceChat', () => {
  it('opens a user bubble and an assistant turn on start', () => {
    const c = run([start]);
    expect(c.turns.map((t) => t.from)).toEqual(['user', 'assistant']);
    expect(c.model).toBe('qwen');
    expect(isStreaming(c)).toBe(true);
  });

  it('appends text and reasoning deltas to the part they continue', () => {
    const c = run([
      start,
      { kind: 'reasoning', delta: 'hm' },
      { kind: 'reasoning', delta: 'm' },
      { kind: 'text', delta: 'Do' },
      { kind: 'text', delta: 'ne' },
    ]);
    expect(assistant(c.turns).parts).toEqual([
      { type: 'reasoning', text: 'hmm' },
      { type: 'text', text: 'Done' },
    ]);
  });

  it('adds a row per tool event, keeping order against text', () => {
    const c = run([
      start,
      { kind: 'text', delta: 'a' },
      { kind: 'tool', name: 'set_steps', args: { t: 1 }, result: 'ok', ok: true },
      { kind: 'tool', name: 'x', args: 'raw', result: 'refused: no', ok: false },
      { kind: 'text', delta: 'b' },
    ]);
    expect(assistant(c.turns).parts.map((p) => p.type)).toEqual(['text', 'tool', 'tool', 'text']);
  });

  it('closes the turn on done and says what happened', () => {
    const applied = run([start, { kind: 'done', applied: true, label: 'Agent: a beat' }]);
    expect(isStreaming(applied)).toBe(false);
    expect(assistant(applied.turns).parts).toEqual([
      { type: 'note', tone: 'info', text: 'Applied as one undo step: Agent: a beat' },
    ]);
    const none = run([start, { kind: 'done', applied: false, label: '' }]);
    expect(assistant(none.turns).parts[0]).toMatchObject({ text: 'Nothing changed' });
  });

  it('writes stopped and error lines before done', () => {
    const c = run([
      start,
      { kind: 'stopped', reason: 'time' },
      { kind: 'error', message: 'no server' },
      { kind: 'done', applied: false, label: '' },
    ]);
    const texts = assistant(c.turns).parts.map((p) => (p.type === 'note' ? p.text : ''));
    expect(texts).toEqual([
      'Stopped: took longer than the time limit',
      'no server',
      'Nothing changed',
    ]);
  });

  it('shows the retry note and the looping stop as muted lines', () => {
    const c = run([
      start,
      { kind: 'note', text: 'Thinking went in circles; asked again without thinking.' },
      { kind: 'stopped', reason: 'looping' },
    ]);
    expect(assistant(c.turns).parts).toEqual([
      {
        type: 'note',
        tone: 'info',
        text: 'Thinking went in circles; asked again without thinking.',
      },
      { type: 'note', tone: 'warn', text: 'Stopped: it was going in circles' },
    ]);
  });

  it('says a stop by the user kept what was written', () => {
    const c = run([
      start,
      { kind: 'stopped', reason: 'cancelled' },
      { kind: 'done', applied: true, label: 'Agent: a beat' },
    ]);
    expect(assistant(c.turns).parts).toEqual([
      { type: 'note', tone: 'info', text: 'Stopped. Kept what it had written' },
      { type: 'note', tone: 'info', text: 'Applied as one undo step: Agent: a beat' },
    ]);
  });

  it('starts a second reply after the first is done', () => {
    const c = run([
      start,
      { kind: 'done', applied: false, label: '' },
      { ...start, prompt: 'again' },
    ]);
    expect(c.turns).toHaveLength(4);
    expect(new Set(c.turns.map((t) => t.id)).size).toBe(4);
  });

  it('shows a refused send as a closed turn with the error', () => {
    const c = reduceChat(emptyConversation, { kind: 'rejected', prompt: 'x', message: 'busy' });
    expect(isStreaming(c)).toBe(false);
    expect(assistant(c.turns).parts).toEqual([{ type: 'note', tone: 'error', text: 'busy' }]);
  });
});

describe('transcript', () => {
  it('writes every turn with tool arguments and results', () => {
    let chat = reduceChat(emptyConversation, { kind: 'start', prompt: 'a bass', model: 'qwen' });
    chat = reduceChat(chat, { kind: 'text', delta: 'Here it is.' });
    chat = reduceChat(chat, {
      kind: 'tool',
      name: 'set_melody',
      args: { steps: [{ step: 0, note: 29 }] },
      result: 'wrote track 1',
      ok: true,
    });
    chat = reduceChat(chat, { kind: 'done', applied: true, label: 'Agent: a bass' });
    const md = transcript(chat);
    expect(md).toContain('Model: qwen');
    expect(md).toContain('## You\n\na bass');
    expect(md).toContain('**set_melody** (done)');
    expect(md).toContain('"note": 29');
    expect(md).toContain('wrote track 1');
  });
});
