//! Restricted DOM automation scripts executed by the system WebKit content process.

pub(crate) const MAX_OUTPUT_BYTES: usize = 64 * 1024;

const HELPER: &str = r#"
function visible(el){
  if(!el||!el.getBoundingClientRect) return false;
  var s=getComputedStyle(el);
  if(s.display==='none'||s.visibility==='hidden'||Number(s.opacity)===0) return false;
  var r=el.getBoundingClientRect();
  return r.width>0&&r.height>0;
}
function uniqueVisible(sel){
  var nodes;
  try { nodes=document.querySelectorAll(sel); }
  catch (e) { return {error:'选择器无效 / Invalid selector'}; }
  var vis=[];
  for (var i=0;i<nodes.length;i++){ if(visible(nodes[i])) vis.push(nodes[i]); }
  if(!vis.length) return {error:'未找到可见元素 / No visible matching element'};
  if(vis.length>1) return {error:'选择器匹配多个可见元素 / Selector matches multiple visible elements'};
  return {el:vis[0]};
}
function inputType(el){
  var t=String((el.getAttribute&&el.getAttribute('type'))||el.type||'').toLowerCase();
  if(el.tagName==='TEXTAREA') return 'textarea';
  if(el.tagName==='SELECT') return 'select';
  if(el.tagName==='BUTTON') return (t||'button');
  return t||'text';
}
function rejectSecret(el){
  var t=inputType(el);
  return (t==='password'||t==='file')
    ? '不支持密码或文件输入 / Password and file inputs are not allowed'
    : null;
}
function cssEscape(value){
  if(window.CSS&&CSS.escape) return CSS.escape(value);
  return String(value).replace(/[^a-zA-Z0-9_-]/g,'\\$&');
}
function selectorFor(el){
  if(el.id){
    var byId='#'+cssEscape(el.id);
    try { if(document.querySelectorAll(byId).length===1) return byId; } catch (e) {}
  }
  var path=[], cur=el;
  while(cur&&cur.nodeType===1&&cur!==document.documentElement){
    var name=cur.tagName.toLowerCase();
    if(cur.id){ path.unshift(name+'#'+cssEscape(cur.id)); break; }
    var i=1, sib=cur;
    while((sib=sib.previousElementSibling)){ if(sib.tagName===cur.tagName) i++; }
    path.unshift(name+':nth-of-type('+i+')');
    cur=cur.parentElement;
  }
  return path.join('>');
}
function webUrl(href){
  try {
    var u=new URL(href, location.href);
    return (u.protocol==='http:'||u.protocol==='https:') && u.username==='' && u.password==='';
  } catch (e) { return false; }
}
function rejectClick(el){
  var blocked=rejectSecret(el);
  if(blocked) return blocked;
  var a=el.tagName==='A'?el:(el.closest?el.closest('a'):null);
  if(!a) return null;
  if(a.hasAttribute('download')) return '不支持下载 / Downloads are not supported';
  var href=String(a.href||a.getAttribute('href')||'');
  if(!href||!webUrl(href)) return '不支持此链接 / This link is not supported';
  return null;
}
"#;

fn js_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
}

fn truncate_to_bytes(text: &str, keep: usize) -> String {
    let mut keep = keep.min(text.len());
    while keep > 0 && !text.is_char_boundary(keep) {
        keep -= 1;
    }
    text[..keep].to_string()
}

pub(crate) fn read_page_script() -> String {
    format!(
        "(function(){{\n{helper}\nvar MAX={max};\nvar TEXT_MAX=24576;\nvar text=((document.body&&document.body.innerText)||'').slice(0,TEXT_MAX);\nvar links=[], as=document.querySelectorAll('a[href]');\nfor(var i=0;i<as.length&&links.length<80;i++){{\n  var a=as[i];\n  if(!visible(a)) continue;\n  var href=a.href||'';\n  if(!webUrl(href)) continue;\n  links.push({{selector:selectorFor(a),href:href,text:String(a.innerText||'').slice(0,200)}});\n}}\nvar inputs=[], fields=document.querySelectorAll('input,textarea,select');\nfor(var j=0;j<fields.length&&inputs.length<80;j++){{\n  var el=fields[j];\n  if(!visible(el)) continue;\n  var ty=inputType(el);\n  var item={{selector:selectorFor(el),type:ty}};\n  if(ty!=='password'&&ty!=='file') item.value=el.value==null?'':String(el.value).slice(0,500);\n  inputs.push(item);\n}}\nvar buttons=[], btns=document.querySelectorAll('button,input[type=button],input[type=submit],input[type=reset]');\nfor(var k=0;k<btns.length&&buttons.length<80;k++){{\n  var btn=btns[k];\n  if(!visible(btn)) continue;\n  var ty=inputType(btn);\n  buttons.push({{selector:selectorFor(btn),type:ty,text:String(btn.innerText||btn.value||'').slice(0,200)}});\n}}\nvar data={{url:String(location.href||''),title:String(document.title||''),text:text,links:links,inputs:inputs,buttons:buttons}};\nvar out=JSON.stringify({{ok:true,data:data}});\nwhile(out.length>MAX&&data.text.length){{ data.text=data.text.slice(0,Math.max(0,data.text.length-1024)); out=JSON.stringify({{ok:true,data:data}}); }}\nwhile(out.length>MAX&&data.links.length){{ data.links.pop(); out=JSON.stringify({{ok:true,data:data}}); }}\nwhile(out.length>MAX&&data.inputs.length){{ data.inputs.pop(); out=JSON.stringify({{ok:true,data:data}}); }}\nwhile(out.length>MAX&&data.buttons.length){{ data.buttons.pop(); out=JSON.stringify({{ok:true,data:data}}); }}\nif(out.length>MAX) return JSON.stringify({{ok:false,error:'页面内容超过 64KiB / Page snapshot exceeds 64KiB'}});\nreturn out;\n}})();",
        helper = HELPER,
        max = MAX_OUTPUT_BYTES,
    )
}

pub(crate) fn click_script(selector: &str) -> String {
    format!(
        "(function(){{\n{helper}\nvar found=uniqueVisible({sel});\nif(found.error) return JSON.stringify({{ok:false,error:found.error}});\nvar blocked=rejectClick(found.el);\nif(blocked) return JSON.stringify({{ok:false,error:blocked}});\nfound.el.focus();\nfound.el.click();\nreturn JSON.stringify({{ok:true,data:{{clicked:true}}}});\n}})();",
        helper = HELPER,
        sel = js_string(selector),
    )
}

pub(crate) fn type_text_script(selector: &str, text: &str) -> String {
    format!(
        "(function(){{\n{helper}\nvar found=uniqueVisible({sel});\nif(found.error) return JSON.stringify({{ok:false,error:found.error}});\nvar el=found.el;\nvar blocked=rejectSecret(el);\nif(blocked) return JSON.stringify({{ok:false,error:blocked}});\nvar tag=el.tagName;\nif(tag!=='INPUT'&&tag!=='TEXTAREA'&&!el.isContentEditable){{\n  return JSON.stringify({{ok:false,error:'目标不可输入 / Target is not editable'}});\n}}\nel.focus();\nvar value={text};\nif('value' in el){{\n  el.value=value;\n  el.dispatchEvent(new Event('input',{{bubbles:true}}));\n  el.dispatchEvent(new Event('change',{{bubbles:true}}));\n}} else {{\n  el.textContent=value;\n}}\nreturn JSON.stringify({{ok:true,data:{{typed:true}}}});\n}})();",
        helper = HELPER,
        sel = js_string(selector),
        text = js_string(text),
    )
}

pub(crate) fn require_selector(selector: &str) -> Result<(), String> {
    if selector.is_empty() || selector.chars().any(char::is_control) {
        Err("选择器无效 / Invalid selector".into())
    } else {
        Ok(())
    }
}

pub(crate) fn decode_envelope(raw: &str) -> Result<serde_json::Value, String> {
    if raw.len() > MAX_OUTPUT_BYTES {
        return Err("页面内容超过 64KiB / Page snapshot exceeds 64KiB".into());
    }
    let value: serde_json::Value = serde_json::from_str(raw.trim())
        .map_err(|_| "网页脚本返回无效 / The page script returned invalid data".to_string())?;
    if value.get("ok").and_then(|ok| ok.as_bool()) == Some(true) {
        Ok(value
            .get("data")
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    } else {
        Err(value
            .get("error")
            .and_then(|error| error.as_str())
            .unwrap_or("操作失败 / Operation failed")
            .to_string())
    }
}

pub(crate) fn encode_success(data: serde_json::Value) -> Result<String, String> {
    serde_json::to_string(&data)
        .map_err(|_| "无法序列化结果 / Unable to serialize result".to_string())
}

pub(crate) fn finalize_page_json(
    mut data: serde_json::Value,
    url: &str,
    title: &str,
) -> Result<String, String> {
    let obj = data
        .as_object_mut()
        .ok_or_else(|| "网页脚本返回无效 / The page script returned invalid data".to_string())?;
    if !url.is_empty() {
        obj.insert("url".into(), serde_json::Value::String(url.to_string()));
    }
    obj.insert("title".into(), serde_json::Value::String(title.to_string()));
    obj.entry("text")
        .or_insert_with(|| serde_json::Value::String(String::new()));
    obj.entry("links")
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    obj.entry("inputs")
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    obj.entry("buttons")
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    if let Some(links) = obj.get_mut("links").and_then(|value| value.as_array_mut()) {
        links.retain(|link| {
            link.get("href")
                .and_then(|href| href.as_str())
                .is_some_and(|href| url::Url::parse(href).is_ok_and(|url| crate::is_web_url(&url)))
        });
    }
    loop {
        let out = serde_json::to_string(&data)
            .map_err(|_| "无法序列化页面 / Unable to serialize page".to_string())?;
        if out.len() <= MAX_OUTPUT_BYTES {
            return Ok(out);
        }
        let obj = data.as_object_mut().expect("page json object");
        let mut shrunk = false;
        if let Some(text) = obj.get("text").and_then(|value| value.as_str()) {
            if !text.is_empty() {
                let drop = (out.len() - MAX_OUTPUT_BYTES).max(1024);
                let keep = text.len().saturating_sub(drop);
                obj.insert(
                    "text".into(),
                    serde_json::Value::String(truncate_to_bytes(text, keep)),
                );
                shrunk = true;
            }
        }
        if !shrunk {
            if let Some(links) = obj.get_mut("links").and_then(|value| value.as_array_mut()) {
                if !links.is_empty() {
                    links.pop();
                    shrunk = true;
                }
            }
        }
        if !shrunk {
            if let Some(inputs) = obj.get_mut("inputs").and_then(|value| value.as_array_mut()) {
                if !inputs.is_empty() {
                    inputs.pop();
                    shrunk = true;
                }
            }
        }
        if !shrunk {
            if let Some(buttons) = obj
                .get_mut("buttons")
                .and_then(|value| value.as_array_mut())
            {
                if !buttons.is_empty() {
                    buttons.pop();
                    shrunk = true;
                }
            }
        }
        if !shrunk {
            return Err("页面内容超过 64KiB / Page snapshot exceeds 64KiB".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finalize_page_json_truncates_unicode_to_output_budget() {
        let data = serde_json::json!({
            "url": "https://example.com/",
            "title": "示例",
            "text": "测".repeat(40_000),
            "links": [{"selector": "#go", "href": "https://example.com/x", "text": "go"}],
            "inputs": [{"selector": "#q", "type": "text", "value": "hi"}],
            "buttons": [{"selector": "#apply", "type": "button", "text": "Apply"}]
        });
        let out = finalize_page_json(data, "https://example.com/path", "真实标题").unwrap();
        assert!(out.len() <= MAX_OUTPUT_BYTES, "len={}", out.len());
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["url"], "https://example.com/path");
        assert_eq!(parsed["title"], "真实标题");
        let text = parsed["text"].as_str().unwrap();
        assert!(!text.is_empty());
        assert!(text.chars().all(|ch| ch == '测'));
        assert!(
            parsed["buttons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|button| button["text"] == "Apply")
        );
    }

    #[test]
    fn decode_envelope_reports_selector_failures() {
        let error = decode_envelope(
            r#"{"ok":false,"error":"选择器匹配多个可见元素 / Selector matches multiple visible elements"}"#,
        )
        .unwrap_err();
        assert!(error.contains("多个可见元素"));
        assert_eq!(
            encode_success(decode_envelope(r#"{"ok":true,"data":{"clicked":true}}"#).unwrap())
                .unwrap(),
            r#"{"clicked":true}"#
        );
        assert_eq!(
            encode_success(decode_envelope(r#"{"ok":true,"data":{"typed":true}}"#).unwrap())
                .unwrap(),
            r#"{"typed":true}"#
        );
    }
}
