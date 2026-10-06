//! Bounded local suggestions: language keywords and words already in this document.
use std::collections::BTreeSet;
use crate::lsp::CompletionItem;

pub(super) fn items(language:&str,lines:&[String],prefix:&str)->Vec<CompletionItem>{
    let keywords=match language {
        "Rust"=>"fn let mut pub impl struct enum trait use mod match if else loop while for in return self Self async await move const static type where Result Option Some None Ok Err",
        "Python"=>"def class import from as return if elif else for while in is not and or None True False with try except finally raise yield async await lambda pass",
        "Go"=>"func package import var const type struct interface return if else for range go defer chan select case switch map nil true false",
        "JavaScript"|"TypeScript"|"TypeScriptReact"|"JavaScript (Babel)"|"TSX"|"JSX"=>"const let var function return class extends import export default from async await if else for while new this null undefined true false interface type typeof keyof public private readonly",
        "C"|"C++"|"Java"|"C#"|"Kotlin"|"Swift"=>"class struct enum public private protected static const void int char bool double float return if else for while switch case break continue namespace using import include null true false",
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
    for (word,origin) in keywords.split_whitespace().map(|w|(w,"언어 키워드")).chain(sample.split(|c:char|!c.is_alphanumeric()&&c!='_').take(20000).map(|w|(w,"문서의 단어"))) {
        if word.chars().count()<2 || !word.to_lowercase().starts_with(&prefix) || word.to_lowercase()==prefix || !words.insert(word.to_owned()){continue;}
        out.push(CompletionItem{label:word.into(),detail:Some(kiln_common::i18n::tr(origin).into()),kind:Some(if origin=="언어 키워드"{14}else{1}),filter_text:word.into(),sort_text:word.into(),insert_text:word.into(),edit:None,additional_edits:vec![],cursor_offset:None});
        if out.len()>=200{break;}
    }
    out
}
