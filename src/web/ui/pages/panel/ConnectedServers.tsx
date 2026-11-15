/** web/ui/pages/panel/ConnectedServers.tsx — the linked-servers list, which is where a link is cut. See ../../../http-router.md. */
import { Button } from '../../lib/Button.tsx';
import { Icon } from '../../lib/icons.tsx';
import { Section } from '../../lib/Section.tsx';
import type { Peer } from './api.ts';

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

export const ConnectedServers = ({
  peers,
  onUnlink,
  unlinking,
}: {
  peers: Peer[];
  onUnlink: (peer: Peer) => void;
  /** The `pub` being unlinked right now, so its button cannot submit the same peer twice. */
  unlinking: string;
}) => (
  <Section
    title="Connected servers"
    subtitle="Their people appear in Immich's share picker."
    icon="serverNetwork"
  >
    {peers.length === 0 ? (
      <span class="isa-empty">None yet — use “Link a server” above.</span>
    ) : (
      <div class="isa-rows">
        {peers.map(p => (
          <div class="isa-row" key={p.pub}>
            <span class="isa-row-icon">
              <Icon name="serverNetwork" size="22px" />
            </span>
            <div class="isa-row-main">
              <div class="isa-row-title">{p.name}</div>
              <div class="isa-row-sub">
                {p.version ? `v${p.version} · ` : ''}
                {plural(p.people, 'person', 'people')} · {p.sharedToThem} out · {p.sharedToUs} in
              </div>
            </div>
            <div class="isa-row-action">
              <Button
                fill="danger"
                aria-label={`Unlink ${p.name}`}
                disabled={unlinking === p.pub}
                onClick={() => onUnlink(p)}
              >
                <Icon name="linkVariantOff" />
                {unlinking === p.pub ? 'Unlinking…' : 'Unlink'}
              </Button>
            </div>
          </div>
        ))}
      </div>
    )}
  </Section>
);
