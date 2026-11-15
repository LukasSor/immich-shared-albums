/** web/ui/lib/Section.tsx — a titled group on a page, drawn as Immich draws a settings group. See design-system.md. */
import type { ComponentChildren } from 'preact';
import { useId, useState } from 'preact/hooks';
import { Icon, type IconName } from './icons.tsx';

export const Section = ({
  title,
  subtitle,
  icon,
  children,
}: {
  title: string;
  subtitle?: string;
  icon: IconName;
  children?: ComponentChildren;
}) => {
  const [open, setOpen] = useState(true);
  const bodyId = useId();
  return (
    <section class={`isa-section isa-accordion isa-enter${open ? ' isa-accordion--open' : ''}`}>
      <h2 class="isa-section-title">
        <button
          type="button"
          class="isa-accordion-head"
          aria-expanded={open}
          aria-controls={bodyId}
          onClick={() => setOpen(!open)}
        >
          <span class="isa-accordion-icon">
            <Icon name={icon} size="24px" />
          </span>
          <span class="isa-accordion-label">{title}</span>
          <span class="isa-accordion-chevron">
            <Icon name="chevronDown" size="20px" />
          </span>
        </button>
      </h2>
      {subtitle && <p class="isa-section-lede">{subtitle}</p>}
      <div id={bodyId} class="isa-accordion-body" hidden={!open}>
        {children}
      </div>
    </section>
  );
};
