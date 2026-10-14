/**
 * The agent chat panel (roadmap row 24). A conversation with a local model
 * that writes tracks into the open session.
 *
 * Self-contained on purpose: it imports nothing from Shard but the agent
 * bridge and the pure reducer beside it, and takes the rest as props, so it
 * can move into agent-kit's TypeScript half. The chat lives in React state
 * for this run of the app and is never saved.
 */
import { Icons, ToggleGroup, ToggleGroupItem, TooltipProvider } from '@preset.nz/ux-kit';
import {
  ChatMarkdown,
  ChatMessage,
  ChatMessageContent,
  ChatScroller,
  ChatScrollerItem,
  PromptInput,
  Reasoning,
  ReasoningContent,
  ReasoningTrigger,
  Suggestion,
  Suggestions,
  ToolCall,
  ToolCallContent,
  ToolCallHeader,
  ToolCallInput,
  ToolCallOutput,
} from '@preset.nz/ux-kit/chat';
import { type KeyboardEvent, useEffect, useReducer, useRef, useState } from 'react';
import { agentCancel, agentSend, onAgentEvent } from '@/lib/agent';
import { emptyConversation, isStreaming, type Part, reduceChat } from './agentChat';

type Mode = 'quick' | 'considered';
const MODE_KEY = 'shard.agent.mode';

function readMode(): Mode {
  try {
    return localStorage.getItem(MODE_KEY) === 'considered' ? 'considered' : 'quick';
  } catch {
    return 'quick';
  }
}

function PartView({ part, live }: { part: Part; live: boolean }) {
  switch (part.type) {
    case 'reasoning':
      return (
        <Reasoning isStreaming={live}>
          <ReasoningTrigger />
          <ReasoningContent>{part.text}</ReasoningContent>
        </Reasoning>
      );
    case 'tool':
      return (
        <ToolCall>
          <ToolCallHeader name={part.tool.name} state={part.tool.ok ? 'done' : 'refused'} />
          <ToolCallContent>
            <ToolCallInput input={part.tool.args} />
            {part.tool.ok ? (
              <ToolCallOutput output={part.tool.result} />
            ) : (
              <ToolCallOutput errorText={part.tool.result} />
            )}
          </ToolCallContent>
        </ToolCall>
      );
    case 'text':
      return (
        <ChatMessageContent>
          <ChatMarkdown isAnimating={live}>{part.text}</ChatMarkdown>
        </ChatMessageContent>
      );
    case 'note':
      return (
        <p
          className={
            part.tone === 'error'
              ? 'px-1 text-[11px] text-destructive'
              : 'px-1 text-[11px] text-muted-foreground'
          }
        >
          {part.text}
        </p>
      );
  }
}

export function AgentPanel({
  suggestions,
  onApplied,
  collapsed,
  onCollapsedChange,
}: {
  /** Starter prompts, shown while the conversation is empty. */
  suggestions: string[];
  /** A reply wrote tracks: read the session again. */
  onApplied: () => void;
  /** Folded to the prompt line. */
  collapsed: boolean;
  onCollapsedChange: (collapsed: boolean) => void;
}) {
  const [chat, dispatch] = useReducer(reduceChat, emptyConversation);
  const [mode, setMode] = useState<Mode>(readMode);
  const [draft, setDraft] = useState('');
  const onAppliedRef = useRef(onApplied);
  onAppliedRef.current = onApplied;

  useEffect(() => {
    const off = onAgentEvent((e) => {
      dispatch(e);
      if (e.kind === 'done' && e.applied) onAppliedRef.current();
    });
    return () => {
      void off.then((f) => f());
    };
  }, []);

  const streaming = isStreaming(chat);

  const changeMode = (next: Mode) => {
    setMode(next);
    try {
      localStorage.setItem(MODE_KEY, next);
    } catch {
      // Not remembering is fine.
    }
  };

  const send = (text: string) => {
    setDraft('');
    agentSend(text, mode === 'considered').catch((err) =>
      dispatch({ kind: 'rejected', prompt: text, message: String(err) }),
    );
  };

  const stop = () => void agentCancel().catch(() => {});

  // Esc stops a running reply, but only from in here: it keeps its other
  // meanings elsewhere, and a stop here does not also deselect.
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === 'Escape' && streaming) {
      e.preventDefault();
      e.stopPropagation();
      e.nativeEvent.stopImmediatePropagation();
      stop();
    }
  };

  const lastIndex = chat.turns.length - 1;

  return (
    <TooltipProvider>
      <section
        aria-label="Agent"
        className={`flex min-h-0 flex-col ${collapsed ? '' : 'flex-1'}`}
        onKeyDown={onKeyDown}
      >
        <header className="flex shrink-0 items-center gap-1 border-b border-border px-2 py-1">
          <button
            type="button"
            className="flex size-5 items-center justify-center text-muted-foreground hover:text-foreground"
            aria-label={collapsed ? 'Show the conversation' : 'Hide the conversation'}
            aria-expanded={!collapsed}
            onClick={() => onCollapsedChange(!collapsed)}
          >
            {collapsed ? <Icons.CaretRightIcon size={12} /> : <Icons.CaretDownIcon size={12} />}
          </button>
          <h2 className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Agent
          </h2>
          {chat.model && (
            <span className="min-w-0 truncate text-[10px] text-muted-foreground" title={chat.model}>
              {chat.model}
            </span>
          )}
        </header>
        {!collapsed && (
          <ChatScroller className="min-h-0 flex-1" aria-label="Conversation">
            {chat.turns.map((turn, index) => (
              <ChatScrollerItem
                key={turn.id}
                messageId={String(turn.id)}
                scrollAnchor={turn.from === 'user'}
              >
                {turn.from === 'user' ? (
                  <ChatMessage from="user">
                    <ChatMessageContent>{turn.text}</ChatMessageContent>
                  </ChatMessage>
                ) : (
                  <ChatMessage from="assistant">
                    {turn.parts.map((part, i) => (
                      <PartView
                        // biome-ignore lint/suspicious/noArrayIndexKey: parts only ever append
                        key={i}
                        part={part}
                        live={
                          turn.open &&
                          index === lastIndex &&
                          i === turn.parts.length - 1 &&
                          (part.type === 'text' || part.type === 'reasoning')
                        }
                      />
                    ))}
                  </ChatMessage>
                )}
              </ChatScrollerItem>
            ))}
          </ChatScroller>
        )}
        <div className="flex shrink-0 flex-col gap-2 border-t border-border p-2">
          {chat.turns.length === 0 && !collapsed && (
            <Suggestions>
              {suggestions.map((s) => (
                <Suggestion key={s} suggestion={s} onClick={setDraft} />
              ))}
            </Suggestions>
          )}
          <PromptInput
            value={draft}
            onValueChange={setDraft}
            onSubmit={send}
            onStop={stop}
            status={streaming ? 'streaming' : 'idle'}
            tools={
              <ToggleGroup
                size="sm"
                spacing={0}
                variant="outline"
                value={[mode]}
                onValueChange={(next) => next[0] && changeMode(next[0] as Mode)}
              >
                <ToggleGroupItem value="quick">Quick</ToggleGroupItem>
                <ToggleGroupItem value="considered">Considered</ToggleGroupItem>
              </ToggleGroup>
            }
          />
        </div>
      </section>
    </TooltipProvider>
  );
}
