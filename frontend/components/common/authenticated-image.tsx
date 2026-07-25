"use client";

import * as React from "react";
import { ImageOff, Loader2 } from "lucide-react";
import { authFetch } from "@/lib/auth";

type AuthenticatedImageProps = Omit<React.ImgHTMLAttributes<HTMLImageElement>, "src"> & {
  src: string;
};

export function AuthenticatedImage({ src, alt, className, ...props }: AuthenticatedImageProps) {
  const [objectUrl, setObjectUrl] = React.useState("");
  const [failed, setFailed] = React.useState(false);

  React.useEffect(() => {
    const controller = new AbortController();
    let nextObjectUrl = "";
    setObjectUrl("");
    setFailed(false);
    void authFetch(src, { cache: "no-store", signal: controller.signal })
      .then(async (response) => {
        if (!response.ok) throw new Error(`HTTP ${response.status}`);
        const blob = await response.blob();
        if (!blob.type.startsWith("image/")) throw new Error("not an image");
        nextObjectUrl = URL.createObjectURL(blob);
        setObjectUrl(nextObjectUrl);
      })
      .catch(() => {
        if (!controller.signal.aborted) setFailed(true);
      });
    return () => {
      controller.abort();
      if (nextObjectUrl) URL.revokeObjectURL(nextObjectUrl);
    };
  }, [src]);

  if (failed) {
    return (
      <span className={`inline-flex items-center justify-center bg-slate-50 text-slate-400 ${className || ""}`} role="img" aria-label={alt}>
        <ImageOff className="h-5 w-5" />
      </span>
    );
  }
  if (!objectUrl) {
    return (
      <span className={`inline-flex items-center justify-center bg-slate-50 text-slate-400 ${className || ""}`} role="img" aria-label={alt}>
        <Loader2 className="h-5 w-5 animate-spin" />
      </span>
    );
  }
  return <img {...props} src={objectUrl} alt={alt} className={className} />;
}
