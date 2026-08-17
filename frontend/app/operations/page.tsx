import { AppShell } from "@/components/layout/app-shell";
import { OperationsPage } from "@/components/settings/operations-page";

export default function Page() {
  return (
    <AppShell active="operations">
      <OperationsPage />
    </AppShell>
  );
}
