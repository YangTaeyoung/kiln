/** Kiln Remote v1. Native enums preserve serde's exact externally tagged representation.
 * Authorization stays in memory. Never automatically retry POSTs or resend a draft.
 */
export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };
export type Provider = 'claude' | 'codex' | 'antigravity';
export interface ApiError { error: { code: string; message: string } }
export interface Pairing { version: 1; url: string; token: string; expires_unix: number }
export interface Workspace { id: string; name: string }
export interface Bootstrap {
  api_version: 1; host_version: '0.1.0'; host_name: string; workspaces: Workspace[];
  providers: { provider: Provider; available: boolean }[];
  capabilities: { conversations: true; terminals: true; image_upload: true; native_actions: false; structured_grants: false };
  limits: { text_bytes: 120000; snapshot_bytes: 2097152; recent_entries: 200; image_bytes: 20971520; mutation_retention_seconds: 600; mutation_results: 1024 };
}
export interface Descriptor { id: string; provider: Provider; workspace_id: string; account_label: string }
export interface ConversationSummary { descriptor: Descriptor; title: string | null; lifecycle: Lifecycle | null; summary: { partial: boolean; unavailable: string | null } }
export interface ConversationList { conversations: ConversationSummary[]; truncated: boolean }
export type Lifecycle = 'Starting' | 'Ready' | 'Running' | 'Waiting' | 'Disconnected' | 'Failed' | 'Stopped';
export type CommandPhase = 'Queued' | 'Dispatched' | 'Accepted' | 'Rejected' | 'Unconfirmed';
export type ApplicationState = 'Pending' | 'Applied' | 'Acknowledged' | 'Rejected' | 'Cancelled' | 'Unconfirmed';
export interface Attachment { id: string; name: string; mime: string; sha256: string }
export interface Choice { id: string; label: string; description: string }
export interface Model { id: string; label: string }
export interface Question { id: string; text: string; options: Choice[]; multi_select: boolean }
export interface ApprovalRequest { id: string; title: string; questions: Question[]; options: Choice[]; details: JsonValue; respondable: boolean; reason: string | null }
export interface Message { id: string; role: 'User' | 'Assistant' | 'System'; text: string; attachments: Attachment[] }
export interface ToolCall { id: string; name: string; input: JsonValue; input_truncated: boolean; output: string; diff: string; status: 'Pending' | 'Running' | 'Completed' | 'Failed' | 'Cancelled'; presentation: null | { kind: string | null; title: string | null; native_name: string | null } }
export type Entry = { Message: Message } | { Tool: ToolCall };
export type Scope = 'Live' | 'NewConversation' | 'Inherited' | 'Managed' | 'Terminal';
export type NativeSandbox = {type:'readOnly'; networkAccess:boolean} | {type:'workspaceWrite'; networkAccess:boolean; writableRoots:string[]; excludeSlashTmp:boolean; excludeTmpdirEnvVar:boolean} | {type:'dangerFullAccess'} | {type:'externalSandbox';networkAccess:'restricted'|'enabled'};
export interface GranularApproval { sandbox_approval: boolean; rules: boolean; mcp_elicitations: boolean; request_permissions: boolean; skill_approval: boolean }
export type ControlValue = 'Default' | {Text:string} | {Bool:boolean} | {Integer:number} | {Strings:string[]} | {Sandbox:NativeSandbox} | {GranularApproval:GranularApproval};
export type ControlKind = 'Select' | 'Boolean' | 'Text' | {Integer:{min:number;max:number}} | 'Strings' | 'Sandbox' | 'GranularApproval' | {Unsupported:string};
export interface ControlChoice { value:string; label:string; description:string; group:string|null }
export interface ControlOption { mobile_supported:boolean; mobile_reason:string|null; id:string; label:string; description:string; category:string|null; kind:ControlKind; choices:ControlChoice[]; current:ControlValue|null; scope:Scope; available:boolean; reason:string|null; source:string|null; documentation:string|null }
export interface ModelMetadata { id:string; description:string; resolved_model:string|null; efforts:ControlChoice[]; default_effort:string|null; tiers:ControlChoice[]; default_tier:string|null; modalities:string[]; availability:Record<string,ControlValue>; capabilities:Record<string,boolean> }
export interface NativeEntry { interface:Record<string,string>; builtin:boolean|null; enabled:boolean|null; scope:string|null; origin:string|null; model:string|null; aliases:string[]; name:string; description:string; argument_hint:string|null; path:string|null; terminal_only:boolean }
export interface Coverage { subject:string; received:number; retained:number; partial:boolean; reason:string|null }
export interface NativeCapability { id:string; label:string; scope:Scope; available:boolean; description:string; documentation:string }
export interface Outcome { settled:boolean; operation:string; control:string; requested:ControlValue; effective:ControlValue|null; state:ApplicationState; message:string|null }
export interface NativeStatus { subject:string; fields:Record<string,ControlValue>; unavailable:string|null }
export interface Controls {
 coverage:Record<string,Coverage>; options_reported:boolean; models_reported:boolean;
 capabilities:NativeCapability[]; queue:{id:string;text:string;editable:boolean}[]|null;
 options:ControlOption[]; models:ModelMetadata[]; commands:NativeEntry[]|null; skills:NativeEntry[]|null; agents:NativeEntry[]|null; styles:NativeEntry[]|null;
 outcomes:Record<string,Outcome>; status:Record<string,NativeStatus>;
}
export interface Snapshot {
 descriptor:Descriptor; generation:string; sequence:number; lifecycle:Lifecycle; entries:Entry[];
 requests:ApprovalRequest[]; models:Model[]; selected_model:string|null; modes:Choice[]; selected_mode:string|null;
 controls:Controls|{unavailable:string}; commands:Record<string,CommandPhase>; uncertain_deliveries:string[];
 errors:string[]; replay_ready:boolean;
 coverage:{truncated:boolean;entries_retained:number;entry_limit:200;byte_limit:2097152;approval_details_complete:boolean};
}
export type LaunchOptions = 'Inherited' | {Claude:ClaudeLaunch} | {Codex:CodexLaunch} | {Antigravity:AntigravityLaunch};
export interface ClaudeLaunch { max_turns:number|null; max_budget_usd:number|null; auto_compact_window:number|null; system_prompt:string|null; append_system_prompt:string|null; setting_sources:('user'|'project'|'local')[]|null; tools:string[]|null; allowed_tools:string[]; disallowed_tools:string[] }
export interface CodexLaunch { model:string|null; effort:string|null; service_tier:string|null; permissions:string|null; summary:'auto'|'concise'|'detailed'|'none'|null; verbosity:'low'|'medium'|'high'|null; web_search:'disabled'|'cached'|'live'|null; context_window:number|null; auto_compact_token_limit:number|null; developer_instructions:string|null; base_instructions:string|null; ephemeral:boolean|null; workspace_roots:string[] }
export interface AntigravityLaunch { sandbox:boolean; agent:string|null; additional_directories:string[] }
export interface CreateConversation { request_id:string; workspace_id:string; provider:Provider; launch_options:LaunchOptions }
export interface PermissionGrant { entry_indices:number[]; read_indices:number[]; write_indices:number[]; network:boolean; scope:'turn'|'session'; strict_review:boolean }
/** Grant remains representable to retain the native contract, but v1 mobile rejects it explicitly. */
export type Decision = 'Allow'|'Deny'|'Cancel'|{Option:string}|{Grant:PermissionGrant};
export type Discovery = 'Settings'|'Commands'|'Skills'|'Mcp'|'Context'|'Policy'|'Status'|'Goal'|'Queue'|'Memory';
export interface NativeSkillInput { name:string; path:string }
export type ReviewTarget = {type:'uncommittedChanges'}|{type:'baseBranch';branch:string}|{type:'commit';sha:string}|{type:'custom';instructions:string};
export type CodexAction = {SetGoal:{objective:string|null;status:'active'|'paused'|'blocked'|'usageLimited'|'budgetLimited'|'complete'|null;token_budget:number|null}}|'ClearGoal'|{AddQueuedTask:{text:string}}|{UpdateQueuedTask:{id:string;text:string}}|{DeleteQueuedTask:{id:string}}|{StartQueuedTask:{id:string}}|{Review:ReviewTarget}|{UpdateActiveTurn:{turn_id:string;control:string;value:ControlValue}}|{AcceptEffectiveSettings:{operation:string}}|{SetMemoryEnabled:boolean}|'ReloadMcp';
export type NativeAction = 'ReloadSkills'|'ReloadPlugins'|'ReloadStyles'|'Compact'|{Codex:CodexAction};
/** Attachments are opaque uploaded IDs, never native filesystem paths. */
export type Command = {Submit:{text:string;attachments:string[];model:string|null}}|{SubmitWithSkills:{text:string;attachments:string[];model:string|null;skills:NativeSkillInput[]}}|{Respond:{request_id:string;decision:Decision;answers:Record<string,string[]>}}|{Configure:{control:string;value:ControlValue}}|{Discover:Discovery}|{NativeAction:NativeAction}|{SetMode:{mode:string}}|'Interrupt'|'Restart';
export interface CommandRequest { request_id:string; generation:string; command:Command }
export interface Upload { request_id:string; filename:string; mime:string; bytes_base64:string }
export interface PendingReceipt { request_id:string; operation_url:string; retention_seconds:600; state:'pending' }
export interface CommandReceipt { request_id:string; operation_url:string; retention_seconds:600; command_id:string; delivery:'queued' }
export interface UploadReceipt { request_id:string; operation_url:string; retention_seconds:600; attachment:Attachment }
export interface InputReceipt { request_id:string; operation_url:string; retention_seconds:600; delivery:'queued' }
export interface Operation { request_id:string; state:'pending'|'succeeded'|'failed'|'unknown'; result:null|{conversation_id:string;terminal_id:string}|{command_id:string;delivery:'queued'}|{attachment:Attachment}|{delivery:'queued'}; error:null|{code:string;message:string}; retention_seconds:600 }
export interface TerminalSummary { id:string; title:string; exited:number|null }
export interface TerminalList { terminals:TerminalSummary[]; truncated:boolean }
export interface Terminal extends TerminalSummary { text:string; truncated:boolean }
export interface TerminalInput { request_id:string; text:string; enter:boolean }
