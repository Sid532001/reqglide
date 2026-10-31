type IconName = "terminal" | "search" | "send" | "import" | "code" | "panel" | "save" | "shield";

const paths: Record<IconName, string> = {
  terminal: "m5 7 5 5-5 5m8 0h6",
  search: "m21 21-4.4-4.4M19 11a8 8 0 1 1-16 0 8 8 0 0 1 16 0",
  send: "m22 2-7 20-4-9-9-4 20-7ZM22 2 11 13",
  import: "M12 3v12m-4-4 4 4 4-4M4 16v5h16v-5",
  code: "m8 5-6 7 6 7m8-14 6 7-6 7m-3-16-2 18",
  panel: "M3 3h18v18H3ZM9 3v18",
  save: "M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h12l4 4v12a2 2 0 0 1-2 2ZM7 3v6h10V3M7 21v-8h10v8",
  shield: "M12 22s9-4 9-11V5l-9-3-9 3v6c0 7 9 11 9 11Zm-4-11 3 3 5-5",
};

export function AppIcon({ name, className = "" }: { name: IconName; className?: string }) {
  return <svg className={`app-icon ${className}`} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={paths[name]} /></svg>;
}
