interface Props {
  onZoomIn(): void;
  onZoomOut(): void;
  onFit(): void;
}

export function ZoomControls({ onZoomIn, onZoomOut, onFit }: Props) {
  const btn = "h-7 w-[30px] border-0 border-b border-[var(--rule)] bg-[var(--chrome)] text-[var(--on)] text-[15px] max-[820px]:h-9 max-[820px]:w-[38px] max-[820px]:text-lg";
  return (
    <div className="absolute bottom-2.5 right-2.5 flex flex-col overflow-hidden rounded-md border border-[var(--rule)] max-[820px]:bottom-auto max-[820px]:top-[74px]">
      <button className={btn} onClick={onZoomIn} aria-label="Zoom in">
        +
      </button>
      <button className={btn} onClick={onZoomOut} aria-label="Zoom out">
        −
      </button>
      <button className={`${btn} border-b-0 text-[10px] max-[820px]:text-[11px]`} onClick={onFit} aria-label="Fit map">
        fit
      </button>
    </div>
  );
}
