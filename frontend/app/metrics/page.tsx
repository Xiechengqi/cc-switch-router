import { AppShell } from "@/components/layout/app-shell";
import { MetricsPage } from "@/components/metrics/metrics-page";

export default function Page() {
  return (
    <AppShell active="metrics">
      <MetricsPage />
    </AppShell>
  );
}
