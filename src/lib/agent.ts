/**
 * The agent panel's bridge (roadmap row 24): ask a local model for a beat or
 * a line, and it writes tracks into the open session.
 *
 * The model is called from Rust (`src-tauri/src/agent/`), never from here.
 * A reply's writes land together at its end as one undo step, labelled
 * "Agent: <the prompt's first words>", so Cmd+Z takes back the whole reply.
 * The chat is this panel's, for this run of the app only: it is never saved
 * in the document.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

/** Which limit stopped a reply. */
export type AgentStopReason = 'cancelled' | 'time' | 'tokens' | 'turns' | 'looping';

/**
 * What arrives on `agent://event` while a reply runs. Every reply starts with
 * `start` and ends with exactly one `done`. `stopped` or `error` come just
 * before `done` when the reply didn't finish on its own:
 *
 * - `cancelled` (Stop or Esc), `time`, `tokens` and `turns` apply what was
 *   written before the stop: each write is a whole track, and one undo takes
 *   it all back. `cancelled` is shown as "Stopped".
 * - `error` applies nothing: `done.applied` is false.
 *
 * `done.applied` is false too when the reply wrote nothing, or when the
 * tracks changed while it ran (that comes with an `error`). After an applied
 * reply, read the tracker again.
 */
export type AgentEvent =
  /** `model` is the model asked, or empty when none could be (an `error` follows). */
  | { kind: 'start'; prompt: string; model: string; considered: boolean }
  | { kind: 'text'; delta: string }
  | { kind: 'reasoning'; delta: string }
  | {
      kind: 'tool';
      name: string;
      /** The arguments as the model sent them, or as text when they weren't JSON. */
      args: unknown;
      /** What the model was told: a summary, or `refused: …`. */
      result: string;
      ok: boolean;
    }
  | { kind: 'stopped'; reason: AgentStopReason }
  /** A muted line about what the loop did on its own (a retry without thinking). */
  | { kind: 'note'; text: string }
  | { kind: 'error'; message: string }
  | { kind: 'done'; applied: boolean; label: string };

/**
 * Where the model is: an OpenAI-compatible server and a model it serves.
 * `model` empty (or `auto`) asks whatever LM Studio has loaded; a model
 * named but not loaded is refused, never loaded behind your back.
 */
export interface AgentSettings {
  baseUrl: string;
  model: string;
}

export const AGENT_EVENT = 'agent://event';

/**
 * Starts a reply. Quick turns reasoning off (seconds, 60 s at most);
 * `considered` leaves it on (about a minute a track, 4 minutes at most).
 * Refused while a reply is running.
 */
export function agentSend(prompt: string, considered: boolean): Promise<void> {
  return invoke('agent_send', { prompt, considered });
}

/** Stops the running reply. What it wrote so far is applied, as one undo step. */
export function agentCancel(): Promise<void> {
  return invoke('agent_cancel');
}

/** File > Save Agent Session: writes the session and the agent log to a new folder; answers it. */
export function agentDump(): Promise<string> {
  return invoke('agent_dump');
}

/**
 * Puts text on the clipboard, through Rust: `navigator.clipboard` fails
 * silently in the macOS webview. Rejects with a message to show.
 */
export function copyText(text: string): Promise<void> {
  return invoke('copy_text', { text });
}

export function agentSettings(): Promise<AgentSettings> {
  return invoke('agent_settings');
}

/** Saves where the model is; an empty address goes back to LM Studio's. */
export function setAgentSettings(settings: AgentSettings): Promise<AgentSettings> {
  return invoke('set_agent_settings', { settings });
}

/** Calls `on` for every agent event until the returned function is called. */
export function onAgentEvent(on: (e: AgentEvent) => void): Promise<UnlistenFn> {
  return listen<AgentEvent>(AGENT_EVENT, (e) => on(e.payload));
}
