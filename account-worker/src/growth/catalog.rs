//! 任务分类：决定一个任务能不能被自动处理。
//!
//! 这张表是**白名单**，不是黑名单。规则很简单：
//! * 只有能对应到"系统自己就能完成"的动作，才算 `Automatic`；
//! * 需要真实产品操作的，算 `RealAction`，必须等对应适配器上线；
//! * 涉及身份 / 外部账号 / 个人信息的，一律 `Manual`；
//! * **匹配不上一律 `Unknown`**，界面显示「待识别」，不自动执行、不自动接受。
//!
//! 为什么强调这点：任务列表是服务端下发的，随时会加新任务。任何"默认自动"的
//! 设计都等于"服务端加一条任务，AICC 就替用户做一件没被授权的事"。

use serde::Serialize;

use super::models::GrowthTask;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskClass {
    /// 系统管理型：接受任务、查询进度、领取奖励。不产生任何用户行为。
    Automatic,
    /// 真实操作型：需要在 WorkBuddy 里真的做出东西，要有专门适配器。
    RealAction,
    /// 人工：身份认证、外部授权、需要本人确认的事。
    Manual,
    /// 待识别：没见过、匹配不上。默认不处理。
    Unknown,
}

/// 一条分类规则。命中即返回，所以顺序有意义：**先人工，再真实操作，最后自动**。
///
/// 反过来排的后果很直观："绑定微信领取 50 积分"会先命中"领积分"这种宽泛的
/// 自动关键词，然后被自动处理 —— 而它其实需要用户自己授权微信。
struct Rule {
    class: TaskClass,
    keywords: &'static [&'static str],
}

const RULES: &[Rule] = &[
    // 1) 人工：涉及身份、外部平台、个人资料。命中就不许自动化。
    Rule {
        class: TaskClass::Manual,
        keywords: &[
            "实名",
            "身份认证",
            "学生认证",
            "学生证",
            "认证",
            "微信",
            "公众号",
            "关注",
            "扫码关注",
            "绑定",
            "授权",
            "第三方",
            "外部账号",
            "捐款",
            "公益",
            "捐赠",
            "收货地址",
            "地址",
            "手机号",
            "填写资料",
            "完善资料",
            "个人资料",
        ],
    },
    // 2) 真实操作：必须在产品里真的做出对应产物。
    Rule {
        class: TaskClass::RealAction,
        keywords: &[
            "画布",
            "canvas",
            "模板",
            "template",
            "playbook",
            "自动化",
            "automation",
            "工作流",
            "资料库",
            "知识库",
            "library",
            "应用",
            "app",
            "发现",
            "专家",
            "专家团",
            "expert",
            "对话",
            "提问",
            "chat",
            "创建",
            "新建",
            "生成",
            "上传",
            "发布",
            "分享",
        ],
    },
    // 3) 自动：只有纯系统侧动作。任务本身的接受与领奖不算"完成任务"。
    Rule {
        class: TaskClass::Automatic,
        keywords: &["签到", "checkin", "打卡", "登录"],
    },
];

/// 判定一个任务属于哪一类。
///
/// 看三个字段：`task_code`（最稳定）、`title`、完成条件。任一命中即算命中 ——
/// 宁可误判成"需要人工"，也不要漏判成"可以自动"。
pub fn classify(task: &GrowthTask) -> TaskClass {
    let mut haystack = String::new();
    haystack.push_str(&task.code.to_lowercase());
    haystack.push(' ');
    haystack.push_str(&task.title.to_lowercase());
    if let Some(text) = &task.condition {
        haystack.push(' ');
        haystack.push_str(&text.to_lowercase());
    }
    if let Some(text) = &task.description {
        haystack.push(' ');
        haystack.push_str(&text.to_lowercase());
    }
    if let Some(text) = &task.tag {
        haystack.push(' ');
        haystack.push_str(&text.to_lowercase());
    }

    for rule in RULES {
        if rule
            .keywords
            .iter()
            .any(|keyword| haystack.contains(&keyword.to_lowercase()))
        {
            return rule.class;
        }
    }
    TaskClass::Unknown
}

/// 给界面用的一句话说明：为什么这个任务不能自动做。
pub fn explain(class: TaskClass) -> &'static str {
    match class {
        TaskClass::Automatic => "系统可以自动完成",
        TaskClass::RealAction => "需要在 WorkBuddy 里真实操作，暂未开放自动执行",
        TaskClass::Manual => "需要你本人完成，AICC 只提供入口",
        TaskClass::Unknown => "任务类型待识别，AICC 不会自动处理",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(code: &str, title: &str, condition: Option<&str>) -> GrowthTask {
        GrowthTask {
            code: code.to_string(),
            title: title.to_string(),
            description: None,
            condition: condition.map(str::to_string),
            task_type: None,
            tag: None,
            jump_url: None,
            locked: false,
            current: 0,
            target: 0,
            reward_credit: 0,
            reward_energy: 0,
            status: super::super::models::TaskStatus::NotAccepted,
            raw_status: None,
        }
    }

    #[test]
    fn identity_and_external_accounts_are_manual() {
        for (code, title) in [
            ("realname", "完成实名认证"),
            ("wechat_follow", "关注公众号"),
            ("bind_phone", "绑定手机号"),
            ("donate", "参与公益捐款"),
            ("address", "填写收货地址"),
        ] {
            assert_eq!(
                classify(&task(code, title, None)),
                TaskClass::Manual,
                "{title} 必须留给人工"
            );
        }
    }

    /// 最容易出错的一条：标题里同时有"微信"和"积分"时，
    /// 人工规则必须先生效，不能被"领积分"拐去自动执行。
    #[test]
    fn manual_wins_over_broader_automatic_keywords() {
        let t = task(
            "bind_wechat_reward",
            "绑定微信领取 50 积分",
            Some("奖励 50 积分"),
        );
        assert_eq!(classify(&t), TaskClass::Manual);
    }

    #[test]
    fn real_actions_are_not_automatic() {
        for (code, title) in [
            ("create_canvas", "创建第一个画布"),
            ("use_template", "使用模板做个东西"),
            ("create_playbook", "创建一个 Playbook"),
            ("explore_library", "体验资料库"),
            ("summon_expert", "召唤一位专家"),
            ("chat_ai", "与 AI 对话 3 次"),
        ] {
            assert_eq!(
                classify(&task(code, title, None)),
                TaskClass::RealAction,
                "{title} 需要真实操作适配器"
            );
        }
    }

    #[test]
    fn unmatched_tasks_are_unknown_not_automatic() {
        let t = task("mystery_2027", "神秘任务", None);
        assert_eq!(classify(&t), TaskClass::Unknown);
    }

    #[test]
    fn classification_reads_the_condition_too() {
        // 标题含糊，但完成条件写清楚了：同样要判成真实操作。
        let t = task("task_a1", "今日任务", Some("在画布中创建一张图"));
        assert_eq!(classify(&t), TaskClass::RealAction);
    }
}
