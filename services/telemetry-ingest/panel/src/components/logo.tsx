export function Logo({ size = 28 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 200 200" aria-hidden="true" className="shrink-0">
      <rect width="200" height="200" rx="44" fill="#1b1d1f" />
      <g fill="none" stroke="#f5f5f0" strokeLinecap="round" strokeWidth="17" transform="translate(0 16)">
        <path d="M 33.49 132.44 A 74 74 0 1 1 166.51 132.44" /><path d="M 56.96 109.15 A 44 44 0 1 1 143.04 109.15" />
      </g>
    </svg>
  );
}
