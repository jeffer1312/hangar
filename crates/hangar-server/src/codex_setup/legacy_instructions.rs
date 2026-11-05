//! Retirada do bloco gerenciado antigo preserva os bytes das instruções pessoais.
use super::phase::Error;
pub fn remove(text:&str)->Result<String,Error>{
 const START:&str="<!-- hangar:codex-instrucoes:start -->";const END:&str="<!-- hangar:codex-instrucoes:end -->";
 let mut remaining=text.to_owned();
 while remaining.contains(START)||remaining.contains(END){
  let invalid=||Error{kind:"ValueError",message:"Bloco de instruções do Hangar incompleto ou malformado".into()};
  let start=remaining.find(START).ok_or_else(invalid)?;let end=remaining.find(END).ok_or_else(invalid)?;
  let nested=remaining[start+START.len()..].find(START).map(|i|i+start+START.len());
  if end<start||nested.is_some_and(|next|next<end){return Err(invalid())}
  let mut end=end+END.len();if remaining[end..].starts_with("\n\n"){end+=2;}remaining.replace_range(start..end,"");
 }Ok(remaining)
}
#[cfg(test)]
mod fuzz{
 use proptest::prelude::*;
 proptest!{#![proptest_config(ProptestConfig::with_cases(48))]
 #[test] fn codex_service_legacy_instructions_text_never_panics(text in ".{0,1024}"){if let Ok(value)=super::remove(&text){prop_assert_eq!(super::remove(&value).unwrap(),value);}}
 }
}
