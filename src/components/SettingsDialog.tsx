import { PropertyPanel } from '@preset.nz/facets';
import { useEffect, useId, useState } from 'react';
import type { MappingsView } from '@/audio';
import { ControllersPanel } from '@/components/ControllersPanel';
import type { ParamRowContext } from '@/components/ParamRow';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { type AgentSettings, agentSettings, setAgentSettings } from '@/lib/agent';
import { type ParamValues, scopeKeyFor } from '@/scope';

/**
 * Where the agent's model is. Read when the dialog opens, saved when a field
 * is left (or Enter is pressed); Rust cleans the values and says what it kept.
 */
function AgentFields({ open, onError }: { open: boolean; onError: (message: string) => void }) {
  const [saved, setSaved] = useState<AgentSettings | null>(null);
  const [baseUrl, setBaseUrl] = useState('');
  const [model, setModel] = useState('');
  const urlId = useId();
  const modelId = useId();

  useEffect(() => {
    if (!open) return;
    agentSettings()
      .then((s) => {
        setSaved(s);
        setBaseUrl(s.baseUrl);
        setModel(s.model);
      })
      .catch((e) => onError(String(e)));
  }, [open, onError]);

  const commit = () => {
    if (!saved || (baseUrl === saved.baseUrl && model === saved.model)) return;
    setAgentSettings({ baseUrl, model })
      .then((s) => {
        setSaved(s);
        setBaseUrl(s.baseUrl);
        setModel(s.model);
      })
      .catch((e) => onError(String(e)));
  };
  const onEnter = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') commit();
  };

  return (
    <div className="mt-1 space-y-2 px-3">
      <div className="text-xs">
        <label htmlFor={urlId} className="text-muted-foreground">
          Base URL
        </label>
        <Input
          id={urlId}
          value={baseUrl}
          placeholder="http://localhost:1234/v1"
          onChange={(e) => setBaseUrl(e.target.value)}
          onBlur={commit}
          onKeyDown={onEnter}
          disabled={!saved}
          spellCheck={false}
        />
      </div>
      <div className="text-xs">
        <label htmlFor={modelId} className="text-muted-foreground">
          Model
        </label>
        <Input
          id={modelId}
          value={model}
          placeholder="Whatever is loaded in LM Studio"
          onChange={(e) => setModel(e.target.value)}
          onBlur={commit}
          onKeyDown={onEnter}
          disabled={!saved}
          spellCheck={false}
        />
      </div>
      <p className="text-xs text-muted-foreground">
        Sampling: Qwen&apos;s recommended values (edit agent.json to change them)
      </p>
    </div>
  );
}

/**
 * Settings, on Cmd+,. Today it holds the tape's feel: how long the brake and a
 * reverse flick take (Georg, 2026-09-14). Brake and reverse themselves stay
 * on the header, because they are played.
 *
 * The values are still the patch's own. Roadmap row 4 makes them app-wide
 * defaults a patch can override, once the shared preferences store exists.
 *
 * Controllers are app-wide already: the MIDI devices seen and the controls
 * learned from them, in `controllers.json`.
 */
export function SettingsDialog({
  open,
  onOpenChange,
  values,
  ctx,
  midi,
  onError,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  values: ParamValues;
  ctx: ParamRowContext;
  midi: MappingsView;
  onError: (message: string) => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Settings</DialogTitle>
          <DialogDescription>
            Tape feel is saved with the patch for now. Controllers and the agent's model belong to
            this Mac.
          </DialogDescription>
        </DialogHeader>
        <div className="-mx-3">
          <div className="px-3 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Tape
          </div>
          <PropertyPanel scopeKey={scopeKeyFor('tape')} selection={values} ctx={ctx} />
          <div className="mt-4 px-3 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Controllers
          </div>
          <div className="mt-1">
            <ControllersPanel open={open} midi={midi} onError={onError} />
          </div>
          <div className="mt-4 px-3 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Agent
          </div>
          <AgentFields open={open} onError={onError} />
        </div>
      </DialogContent>
    </Dialog>
  );
}
