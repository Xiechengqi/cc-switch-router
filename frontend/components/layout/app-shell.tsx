"use client";

import Image from "next/image";
import Link from "next/link";
import { LogOut, Settings, UserRound } from "lucide-react";
import * as React from "react";
import { LoginDialog } from "@/components/auth/login-dialog";
import { AuthProvider, useAuth } from "@/components/auth/auth-provider";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

function Topbar({ active }: { active: "dashboard" | "settings" }) {
  const { session, loading, logout } = useAuth();
  const [loginOpen, setLoginOpen] = React.useState(false);
  const authed = !!session?.authenticated;

  return (
    <header className="mx-auto flex w-[calc(100%-2rem)] max-w-7xl items-center justify-between gap-4 py-5">
      <Link href="/" className="flex items-center gap-3">
        <Image src="/router-logo.svg" alt="" width={36} height={36} className="h-9 w-9" priority />
        <span className="grid gap-0.5">
          <span className="text-base font-extrabold leading-none">Switch Router</span>
          <span className="mono-label text-muted-foreground">Control Plane</span>
        </span>
      </Link>
      <nav className="hidden items-center gap-2 md:flex">
        <Button asChild variant={active === "dashboard" ? "secondary" : "ghost"} size="sm">
          <Link href="/">Dashboard</Link>
        </Button>
        <Button asChild variant={active === "settings" ? "secondary" : "ghost"} size="sm">
          <Link href="/settings/">Settings</Link>
        </Button>
      </nav>
      <div className="flex items-center gap-2">
        {authed ? (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="outline" size="sm">
                <UserRound className="h-4 w-4" />
                <span className="hidden max-w-48 truncate sm:inline">{session?.user?.email}</span>
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuLabel>{session?.user?.email}</DropdownMenuLabel>
              <DropdownMenuSeparator />
              {session?.isAdmin ? (
                <DropdownMenuItem asChild>
                  <Link href="/settings/">
                    <Settings className="h-4 w-4" />
                    Settings
                  </Link>
                </DropdownMenuItem>
              ) : null}
              <DropdownMenuItem onClick={() => logout().catch(console.error)} className="text-destructive">
                <LogOut className="h-4 w-4" />
                Logout
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        ) : (
          <Button variant="outline" size="sm" onClick={() => setLoginOpen(true)} disabled={loading}>
            Login
          </Button>
        )}
      </div>
      <LoginDialog open={loginOpen} onOpenChange={setLoginOpen} />
    </header>
  );
}

export function AppShell({
  active,
  children,
}: {
  active: "dashboard" | "settings";
  children: React.ReactNode;
}) {
  return (
    <AuthProvider>
      <Topbar active={active} />
      {children}
    </AuthProvider>
  );
}
