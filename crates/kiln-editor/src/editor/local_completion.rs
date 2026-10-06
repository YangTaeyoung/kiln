//! Bounded local suggestions: language keywords and words already in this document.
use std::collections::BTreeSet;
use crate::lsp::CompletionItem;

pub(super) fn items(language:&str,lines:&[String],prefix:&str)->Vec<CompletionItem>{
    let keywords=match language {
        "Rust"=>"fn let mut pub impl struct enum trait use mod match if else loop while for in return self Self async await move const static type where Result Option Some None Ok Err",
        "Python"=>"def class import from as return if elif else for while in is not and or None True False with try except finally raise yield async await lambda pass",
        "Go"=>"func package import var const type struct interface return if else for range go defer chan select case switch map nil true false",
        "JavaScript"|"JavaScript (Babel)"|"JSX"=>"const let var function return class extends import export default from async await if else for while new this null undefined true false typeof instanceof switch case break continue try catch finally throw yield delete void super",
        "TypeScript"|"TypeScriptReact"|"TSX"=>"const let var function return class extends import export default from async await if else for while new this null undefined true false typeof instanceof switch case break continue try catch finally throw yield delete void super interface type keyof public private protected readonly implements abstract declare namespace infer unknown never satisfies",
        "C"=>"auto break case char const continue default do double else enum extern float for goto if int long register return short signed sizeof static struct switch typedef union unsigned void volatile while",
        "C++"=>"class struct enum public private protected static const constexpr consteval constinit void int char bool double float return if else for while switch case break continue namespace using template typename new delete nullptr true false virtual override final try catch throw this auto decltype sizeof union unsigned signed inline explicit operator friend volatile mutable co_await co_return co_yield concept requires",
        "Java"=>"abstract assert boolean break byte case catch char class continue default do double else enum extends final finally float for if implements import instanceof int interface long native new package private protected public return short static super switch synchronized this throw throws transient try void volatile while record sealed permits var yield null true false",
        "C#"=>"abstract as base bool break byte case catch char checked class const continue decimal default delegate do double else enum event explicit extern false finally fixed float for foreach if implicit in int interface internal is lock long namespace new null object operator out override params private protected public readonly ref return sbyte sealed short sizeof stackalloc static string struct switch this throw true try typeof uint ulong unchecked unsafe ushort using virtual void volatile while async await record required init get set var partial where yield",
        "Kotlin"=>"as break class continue do else false for fun if in interface is null object package return super this throw true try typealias val var when while by catch constructor finally get import init set where abstract annotation companion const data enum external final infix inline inner internal lateinit open operator out override private protected public reified sealed suspend tailrec vararg",
        "Swift"=>"associatedtype class deinit enum extension fileprivate func import init inout internal let open operator private protocol public rethrows static struct subscript typealias var break case catch continue default defer do else fallthrough for guard if in repeat return throw switch where while as Any is nil self Self super throws try async await actor some any true false get set willSet didSet mutating nonmutating override required convenience weak unowned lazy",
        "SQL"=>"SELECT FROM WHERE JOIN ON GROUP ORDER BY HAVING INSERT INTO VALUES UPDATE SET DELETE CREATE TABLE ALTER DROP NULL AS AND OR NOT LIMIT",
        "JSON"=>"true false null", "Lua"=>"local function end if then else elseif for while do return nil true false require",
        "Ruby"=>"def end class module require if else elsif unless do return nil true false attr_reader attr_accessor",
        "Bourne Again Shell (bash)"=>"if then else elif fi for do done while case esac function export local readonly return echo printf",
        "HTML"=>"html head body title meta link script style div span section main header footer button input form label",
        "CSS"=>"color background display position margin padding width height border flex grid font-size justify-content align-items",
        "YAML"|"TOML"=>"true false", _=>"",
    };
    let prefix=prefix.to_lowercase();
    let sample: String=lines.iter().take(2000).flat_map(|l|l.chars().chain(std::iter::once('\n'))).take(65536).collect();
    let mut words=BTreeSet::new();let mut out=Vec::new();
    for (word,origin) in keywords.split_whitespace().map(|w|(w,"언어 제안")).chain(sample.split(|c:char|!c.is_alphanumeric()&&c!='_').take(20000).map(|w|(w,"문서의 단어"))) {
        if word.chars().count()<2 || !word.to_lowercase().starts_with(&prefix) || word.to_lowercase()==prefix || !words.insert(word.to_owned()){continue;}
        out.push(CompletionItem{label:word.into(),detail:Some(kiln_common::i18n::tr(origin).into()),kind:Some(1),filter_text:word.into(),sort_text:word.into(),insert_text:word.into(),edit:None,additional_edits:vec![],cursor_offset:None});
        if out.len()>=200{break;}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_uses_the_selected_language_without_foreign_keywords() {
        for (language, expected, foreign) in [
            ("C", "typedef", "class"),
            ("C++", "namespace", "package"),
            ("Java", "implements", "struct"),
            ("C#", "foreach", "package"),
            ("Kotlin", "fun", "struct"),
            ("Swift", "func", "namespace"),
            ("JavaScript", "function", "interface"),
            ("TypeScript", "interface", "struct"),
        ] {
            let suggestions=items(language,&[],"");
            assert!(suggestions.iter().any(|item|item.label==expected),"missing {language}: {expected}");
            assert!(!suggestions.iter().any(|item|item.label==foreign),"foreign keyword in {language}: {foreign}");
        }
        let words=items("Java", &["struct_notes".into()], "str");
        assert_eq!(words.len(),1);
        assert_eq!(words[0].label,"struct_notes");
        assert_eq!(words[0].kind,Some(1),"document words remain available without being labelled language keywords");
    }
}
