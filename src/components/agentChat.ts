/**
 * The agent panel's conversation, folded from `agent://event`s. Pure, so it
 * is testable without a webview; `AgentPanel` holds the state and feeds it.
 *
 * A reply is one assistant turn: parts in the order they arrived (text,
 * reasoning, tool calls, status lines). Nothing here is saved anywhere.
 */
import type { AgentEvent, AgentStopReason } from '@/lib/agent';

export type ToolRow = {
  name: string;
  args: unknown;
  result: string;
  ok: boolean;
};

export type Part =
  | { type: 'text'; text: string }
  | { type: 'reasoning'; text: string }
  | { type: 'tool'; tool: ToolRow }
  /** A muted line: why a reply stopped, an error, or what became of it. */
  | { type: 'note'; tone: 'info' | 'warn' | 'error'; text: string };

export type Turn =
  | { id: number; from: 'user'; text: string }
  | { id: number; from: 'assistant'; parts: Part[]; open: boolean };

export interface Conversation {
  turns: Turn[];
  /** The model the last reply asked, for the header. Empty when unknown. */
  model: string;
  nextId: number;
}

export const emptyConversation: Conversation = { turns: [], model: '', nextId: 1 };

/** An event, or the panel's own: a send the backend refused to start. */
export type ChatAction = AgentEvent | { kind: 'rejected'; prompt: string; message: string };

export function stopText(reason: AgentStopReason): string {
  switch (reason) {
    case 'cancelled':
      return 'Stopped. Kept what it had written';
    case 'time':
      return 'Stopped: took longer than the time limit';
    case 'tokens':
      return 'Stopped: used up its tokens';
    case 'turns':
      return 'Stopped: used up its turns';
    case 'looping':
      return 'Stopped: it was going in circles';
  }
}

/** True from `start` until `done`. */
export function isStreaming(c: Conversation): boolean {
  const last = c.turns[c.turns.length - 1];
  return last?.from === 'assistant' && last.open;
}

function withUser(c: Conversation, prompt: string, open: boolean): Conversation {
  return {
    ...c,
    turns: [
      ...c.turns,
      { id: c.nextId, from: 'user', text: prompt },
      { id: c.nextId + 1, from: 'assistant', parts: [], open },
    ],
    nextId: c.nextId + 2,
  };
}

/** Changes the open assistant turn, making one if events arrive without a `start`. */
function editOpen(c: Conversation, change: (parts: Part[]) => Part[]): Conversation {
  const last = c.turns[c.turns.length - 1];
  if (last?.from === 'assistant' && last.open) {
    const turn = { ...last, parts: change(last.parts) };
    return { ...c, turns: [...c.turns.slice(0, -1), turn] };
  }
  const turn: Turn = { id: c.nextId, from: 'assistant', parts: change([]), open: true };
  return { ...c, turns: [...c.turns, turn], nextId: c.nextId + 1 };
}

function append(parts: Part[], type: 'text' | 'reasoning', delta: string): Part[] {
  const last = parts[parts.length - 1];
  if (last?.type === type) {
    return [...parts.slice(0, -1), { type, text: last.text + delta }];
  }
  return [...parts, { type, text: delta }];
}

export function reduceChat(c: Conversation, action: ChatAction): Conversation {
  switch (action.kind) {
    case 'start':
      return { ...withUser(c, action.prompt, true), model: action.model || c.model };
    case 'text':
      return editOpen(c, (p) => append(p, 'text', action.delta));
    case 'reasoning':
      return editOpen(c, (p) => append(p, 'reasoning', action.delta));
    case 'tool':
      return editOpen(c, (p) => [
        ...p,
        {
          type: 'tool',
          tool: { name: action.name, args: action.args, result: action.result, ok: action.ok },
        },
      ]);
    case 'stopped':
      return editOpen(c, (p) => [
        ...p,
        {
          type: 'note',
          tone: action.reason === 'cancelled' ? 'info' : 'warn',
          text: stopText(action.reason),
        },
      ]);
    case 'note':
      return editOpen(c, (p) => [...p, { type: 'note', tone: 'info', text: action.text }]);
    case 'error':
      return editOpen(c, (p) => [...p, { type: 'note', tone: 'error', text: action.message }]);
    case 'done': {
      const closed = editOpen(c, (p) => [
        ...p,
        {
          type: 'note',
          tone: 'info',
          text: action.applied ? `Applied as one undo step: ${action.label}` : 'Nothing changed',
        },
      ]);
      const last = closed.turns[closed.turns.length - 1];
      if (last?.from !== 'assistant') return closed;
      return { ...closed, turns: [...closed.turns.slice(0, -1), { ...last, open: false }] };
    }
    case 'rejected': {
      const next = withUser(c, action.prompt, false);
      const last = next.turns[next.turns.length - 1];
      if (last?.from !== 'assistant') return next;
      const turn = {
        ...last,
        parts: [{ type: 'note' as const, tone: 'error' as const, text: action.message }],
      };
      return { ...next, turns: [...next.turns.slice(0, -1), turn] };
    }
  }
}

/** The conversation as Markdown, for the clipboard: every turn, with each
 * tool call's arguments and result, so a reply can be pasted into a bug
 * report or an eval note as it happened. */
export function transcript(chat: Conversation): string {
  const out: string[] = [];
  if (chat.model) out.push(`Model: ${chat.model}`, '');
  for (const turn of chat.turns) {
    if (turn.from === 'user') {
      out.push(`## You`, '', turn.text, '');
      continue;
    }
    out.push('## Agent', '');
    for (const part of turn.parts) {
      switch (part.type) {
        case 'text':
          out.push(part.text, '');
          break;
        case 'reasoning':
          out.push('<details><summary>Thinking</summary>', '', part.text, '', '</details>', '');
          break;
        case 'tool':
          out.push(
            `**${part.tool.name}** ${part.tool.ok ? '(done)' : '(refused)'}`,
            '',
            '```json',
            JSON.stringify(part.tool.args, null, 2),
            '```',
            '',
            '```',
            part.tool.result,
            '```',
            '',
          );
          break;
        case 'note':
          out.push(`_${part.text}_`, '');
          break;
      }
    }
  }
  return `${out.join('\n').trimEnd()}\n`;
}
