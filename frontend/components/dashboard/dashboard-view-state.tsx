"use client";

import * as React from "react";

type DashboardViewStateValue = {
  issuesOnly: boolean;
  setIssuesOnly: (value: boolean) => void;
};

const DashboardViewStateContext = React.createContext<DashboardViewStateValue | null>(null);

export function DashboardViewStateProvider({ children }: { children: React.ReactNode }) {
  const [issuesOnly, setIssuesOnly] = React.useState(false);
  const value = React.useMemo(() => ({ issuesOnly, setIssuesOnly }), [issuesOnly]);
  return <DashboardViewStateContext.Provider value={value}>{children}</DashboardViewStateContext.Provider>;
}

export function useDashboardViewState() {
  const value = React.useContext(DashboardViewStateContext);
  if (!value) throw new Error("useDashboardViewState must be used inside DashboardViewStateProvider");
  return value;
}
