/** Run `tick` on a timer only while the tab is visible. Hiding clears the
 * timer; showing runs once immediately and starts it again. */
export function bindVisibleInterval(tick: () => void, intervalMs: number) {
  let timer = 0;
  const stop = () => {
    window.clearInterval(timer);
    timer = 0;
  };
  const start = () => {
    if (timer || document.visibilityState === "hidden") return;
    tick();
    timer = window.setInterval(tick, intervalMs);
  };
  const onVisibility = () => {
    if (document.visibilityState === "hidden") stop();
    else start();
  };
  start();
  document.addEventListener("visibilitychange", onVisibility);
  return () => {
    document.removeEventListener("visibilitychange", onVisibility);
    stop();
  };
}
