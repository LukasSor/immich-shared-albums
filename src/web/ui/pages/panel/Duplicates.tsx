/** web/ui/pages/panel/Duplicates.tsx — scanning for duplicate mirror albums and removing them by hand. See docs/sync-loops.md. */
import { useState } from 'preact/hooks';
import { Button } from '../../lib/Button.tsx';
import type { Confirmation } from '../../lib/confirm.tsx';
import { Icon } from '../../lib/icons.tsx';
import type { Outcome } from '../../lib/announce.ts';
import { Section } from '../../lib/Section.tsx';
import { cleanDuplicates, scanDuplicates, type DuplicateAlbum } from './api.ts';

const day = (createdAt: string) => createdAt.slice(0, 10);

export const Duplicates = ({
  onOutcome,
  onConfirm,
}: {
  onOutcome: (outcome: Outcome) => void;
  onConfirm: (ask: Confirmation) => void;
}) => {
  const [found, setFound] = useState<DuplicateAlbum[] | null>(null);
  const [busy, setBusy] = useState(false);

  const scan = async () => {
    setBusy(true);
    try {
      const scanned = await scanDuplicates();
      setFound(scanned.duplicates);
      if (scanned.duplicates.length === 0) onOutcome({ kind: 'ok', text: 'No duplicate albums found.' });
    } catch (err) {
      onOutcome({ kind: 'error', text: `Could not scan: ${(err as Error).message}` });
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    setBusy(true);
    try {
      const outcome = await cleanDuplicates();
      setFound(null);
      const failed = outcome.failed > 0 ? ` ${outcome.failed} could not be removed.` : '';
      const more = outcome.heldBack > 0 ? ` ${outcome.heldBack} more are left — run it again.` : '';
      onOutcome({
        kind: outcome.failed > 0 ? 'error' : 'ok',
        text: `Removed ${outcome.removed} duplicate album${outcome.removed === 1 ? '' : 's'}.${failed}${more}`,
      });
    } catch (err) {
      onOutcome({ kind: 'error', text: `Could not remove them: ${(err as Error).message}` });
    } finally {
      setBusy(false);
    }
  };

  return (
    <Section
      title="Duplicate albums"
      subtitle="Mirror albums left behind with nothing tracking them, beside the live copy of the same share."
      icon="contentDuplicate"
    >
      <div class="isa-actions">
        <Button fill="outlined" disabled={busy} onClick={scan}>
          <Icon name="contentDuplicate" />
          {busy && found === null ? 'Scanning…' : 'Scan for duplicates'}
        </Button>
      </div>
      {found !== null && found.length === 0 && <p class="isa-empty">Nothing to remove.</p>}
      {found !== null && found.length > 0 && (
        <>
          <div class="isa-rows">
            {found.map(duplicate => (
              <div class="isa-row" key={duplicate.albumId}>
                <span class="isa-row-icon">
                  <Icon name="imageAlbum" size="22px" />
                </span>
                <div class="isa-row-main">
                  <div class="isa-row-title">{duplicate.name}</div>
                  <div class="isa-row-sub">
                    a copy of {duplicate.owner}'s share, created {day(duplicate.createdAt)}
                  </div>
                </div>
              </div>
            ))}
          </div>
          <div class="isa-stack">
            <div class="isa-actions">
              <Button
                fill="dangerFilled"
                disabled={busy}
                onClick={() =>
                  onConfirm({
                    title: `Remove ${found.length} duplicate album${found.length === 1 ? '' : 's'}?`,
                    body: 'Only these albums are deleted. The live copy of each share, and every photo, stays.',
                    confirm: 'Remove',
                    danger: true,
                    onConfirm: () => void remove(),
                  })
                }
              >
                <Icon name="deleteSweepOutline" />
                Remove {found.length} duplicate{found.length === 1 ? '' : 's'}
              </Button>
            </div>
          </div>
        </>
      )}
    </Section>
  );
};
