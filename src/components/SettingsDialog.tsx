import { PropertyPanel } from '@preset.nz/facets';
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
import { type ParamValues, scopeKeyFor } from '@/scope';

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
            Tape feel is saved with the patch for now. Controllers belong to this Mac.
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
        </div>
      </DialogContent>
    </Dialog>
  );
}
