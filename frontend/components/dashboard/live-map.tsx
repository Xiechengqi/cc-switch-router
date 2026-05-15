"use client";

import Image from "next/image";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent } from "@/components/ui/card";
import type { DashboardResponse, MapPoint } from "@/lib/types";
import { cn } from "@/lib/utils";

function projectPoint(point: MapPoint) {
  if (typeof point.lat !== "number" || typeof point.lon !== "number") return null;
  const x = ((point.lon + 180) / 360) * 100;
  const y = ((90 - point.lat) / 180) * 100;
  return { x: Math.max(1, Math.min(99, x)), y: Math.max(1, Math.min(99, y)) };
}

export function LiveMap({ data }: { data: DashboardResponse | null }) {
  const clients = data?.map?.clients || [];
  const server = data?.map?.server;
  const points = [server, ...clients].filter(Boolean) as MapPoint[];

  return (
    <Card className="surface-elevated overflow-hidden rounded-lg">
      <CardContent className="p-0">
        <div className="flex flex-wrap items-center justify-between gap-3 border-b bg-white/70 px-5 py-4">
          <div>
            <div className="section-label">Live Network</div>
            <h1 className="mt-3 font-display text-3xl leading-tight md:text-5xl">
              Router traffic <span className="gradient-text">surface</span>
            </h1>
          </div>
          <div className="flex flex-wrap gap-2">
            <Badge variant="secondary">{clients.length} clients mapped</Badge>
            <Badge variant={data?.stats?.totalActiveRequests ? "success" : "outline"}>
              {data?.stats?.totalActiveRequests || 0} in-flight
            </Badge>
          </div>
        </div>
        <div className="relative aspect-[2/1] min-h-[320px] overflow-hidden bg-slate-950">
          <Image src="/world-map.svg" alt="" fill className="object-cover opacity-45 invert" priority />
          <div className="absolute inset-0 bg-[radial-gradient(circle_at_50%_45%,rgba(0,82,255,0.22),transparent_35%),linear-gradient(180deg,rgba(15,23,42,0.1),rgba(15,23,42,0.55))]" />
          {points.map((point) => {
            const pos = projectPoint(point);
            if (!pos) return null;
            const isServer = point.pointType === "server";
            return (
              <div
                key={`${point.pointType}-${point.id}`}
                className="absolute -translate-x-1/2 -translate-y-1/2"
                style={{ left: `${pos.x}%`, top: `${pos.y}%` }}
                title={`${point.label} ${point.country || ""}`}
              >
                <div
                  className={cn(
                    "h-3 w-3 rounded-full ring-4",
                    isServer ? "bg-white ring-white/20" : "gradient-accent ring-blue-400/20",
                    point.activeRequests > 0 && "pulse-dot",
                  )}
                />
              </div>
            );
          })}
          {points.length === 0 ? (
            <div className="absolute inset-0 grid place-items-center text-sm text-slate-300">Waiting for mapped clients</div>
          ) : null}
        </div>
      </CardContent>
    </Card>
  );
}
