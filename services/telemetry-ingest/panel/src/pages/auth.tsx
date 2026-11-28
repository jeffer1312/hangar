import { useState, type FormEvent, type ReactNode } from "react";
import { Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { api, HttpError } from "@/lib/api";
import { Logo } from "@/components/logo";

function Shell({ title, lead, children, onSubmit }: { title: string; lead: string; children: ReactNode; onSubmit: (e: FormEvent<HTMLFormElement>) => void }) {
  return (
    <main className="relative grid min-h-dvh place-items-center px-4" style={{ paddingTop: "calc(var(--st) + 24px)", paddingBottom: "calc(var(--sb) + 24px)" }}>
      <div aria-hidden="true" className="pointer-events-none fixed inset-x-0 top-0 h-[60vh] bg-[radial-gradient(55%_55%_at_50%_0%,rgb(124_135_232/0.13),transparent_70%)]" />
      <form noValidate onSubmit={onSubmit} className="relative w-full max-w-[400px] rounded-2xl border border-border-strong bg-card px-6 pt-8 pb-6 shadow-2xl shadow-black/50 sm:px-8 sm:pt-10">
        <div className="mb-8 flex items-center gap-3">
          <Logo size={40} />
          <div><h1 className="text-[19px] font-semibold tracking-[-0.01em]">{title}</h1><p className="text-[13px] text-muted-foreground">{lead}</p></div>
        </div>
        {children}
      </form>
    </main>
  );
}

function Field({ id, label, hint, ...input }: { id: string; label: string; hint?: string } & React.ComponentProps<"input">) {
  return (
    <div className="mb-5 grid gap-2">
      <Label htmlFor={id} className="text-[13px] font-medium text-muted-foreground">{label}</Label>
      <Input id={id} name={id} className="h-11 bg-deep" {...input} />
      {hint ? <span className="text-[12px] text-faint">{hint}</span> : null}
    </div>
  );
}

const Submit = ({ busy, children, disabled }: { busy: boolean; children: ReactNode; disabled?: boolean }) => (
  <Button type="submit" disabled={busy || disabled} className="mt-1 h-11 w-full text-[14px] font-semibold">{busy ? <Loader2 className="animate-spin" /> : null}{children}</Button>
);
const Msg = ({ text }: { text: string }) => <p role="alert" className="mt-3 min-h-5 text-[13px] text-destructive">{text}</p>;

export function Login({ onDone }: { onDone: () => void }) {
  const [busy, setBusy] = useState(false), [msg, setMsg] = useState("");
  return (
    <Shell title="Telemetria do Hangar" lead="Entre com a sua conta." onSubmit={async e => {
      e.preventDefault();
      const f = e.currentTarget;
      setBusy(true); setMsg("");
      try {
        await api("/panel/api/login", { method: "POST", json: { email: f.email.value, password: f.password.value } });
        onDone();
      } catch (err) {
        setMsg(err instanceof HttpError ? (err.status === 401 ? "E-mail ou senha não conferem." : err.status === 429 ? "Muitas tentativas. Espere alguns minutos e tente de novo."
          : "Não deu para entrar agora (" + err.status + "). Tente de novo.") : (err as Error).message);
        f.password.select();
      } finally { setBusy(false); }
    }}>
      <Field id="email" label="E-mail" type="email" autoComplete="username" required autoFocus />
      <Field id="password" label="Senha" type="password" autoComplete="current-password" required />
      <Submit busy={busy}>{busy ? "Entrando…" : "Entrar"}</Submit>
      <Msg text={msg} />
      <p className="mt-4 text-center text-[12px] text-faint">Sem conta? Peça um convite a quem já tem acesso.</p>
    </Shell>
  );
}

const JOIN_ERRORS: Record<string, string> = {
  email: "Esse e-mail não parece válido.",
  password: "A senha precisa de pelo menos 10 caracteres.",
  invite: "Este convite já foi usado ou venceu. Peça um novo a quem te convidou.",
  email_taken: "Já existe uma conta com esse e-mail. Entre por /panel.",
};

export function Join({ onDone }: { onDone: () => void }) {
  // O convite vem depois do #: não vai ao servidor no carregamento da página nem fica em log de acesso.
  const invite = location.hash.slice(1);
  const valid = /^[0-9a-f]{64}$/.test(invite);
  const [busy, setBusy] = useState(false), [msg, setMsg] = useState(valid ? "" : "Este link de convite está incompleto. Peça um novo a quem te convidou.");
  return (
    <Shell title="Criar sua conta" lead="Você recebeu um convite para o painel da telemetria do Hangar." onSubmit={async e => {
      e.preventDefault();
      const f = e.currentTarget;
      setMsg("");
      if (f.password.value !== f.again.value) return setMsg("As duas senhas não são iguais.");
      if (f.password.value.length < 10) return setMsg(JOIN_ERRORS.password);
      setBusy(true);
      try {
        await api("/panel/api/join", { method: "POST", json: { invite, email: f.email.value, password: f.password.value } });
        history.replaceState(null, "", "/panel");
        onDone();
      } catch (err) {
        setMsg(err instanceof HttpError ? JOIN_ERRORS[err.code ?? ""] ?? "Não deu para criar a conta agora (" + err.status + ")." : (err as Error).message);
      } finally { setBusy(false); }
    }}>
      <Field id="email" label="E-mail" type="email" autoComplete="username" required autoFocus />
      <Field id="password" label="Senha" type="password" autoComplete="new-password" minLength={10} required hint="Pelo menos 10 caracteres." />
      <Field id="again" label="Repita a senha" type="password" autoComplete="new-password" required />
      <Submit busy={busy} disabled={!valid}>{busy ? "Criando…" : "Criar conta e entrar"}</Submit>
      <Msg text={msg} />
    </Shell>
  );
}
