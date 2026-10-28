# mini-D5：对标深度智耀 D5 平台的 AI 药物研发轻量 Demo

> **项目定位**：对标 D5 平台核心能力（知识图谱 + 智能问答）的轻量原型，用于入职前建立医药领域认知。
>
> **技术栈**：Neo4j + Flask + Vue3 + LangChain + FAISS + DeepSeek API
>
> **预计工期**：2 周 / 25-35h
>
> **核心模块**：知识图谱可视化 + RAG 医药文献问答

---

## 一、架构设计

```
┌───────────────────────────────────────────┐
│      Vue3 前端：图谱可视化 + 对话界面       │
└───────────────────────────────────────────┘
                      ↓
┌───────────────────────────────────────────┐
│               Flask API 层                 │
└───────────────────────────────────────────┘
           ↙                    ↘
┌──────────────────┐    ┌──────────────────┐
│   Neo4j 知识图谱  │    │  LangChain RAG   │
│  药物-靶点-疾病   │    │  文献检索+问答    │
└──────────────────┘    └──────────────────┘
```

| 模块 | 职责 | 对应 D5 |
|------|------|---------|
| **Neo4j 知识图谱** | 存储实体与关系，支持图谱浏览 | D5 图谱底座 |
| **LangChain RAG** | 医药文献向量检索 + LLM 问答 | D5 智能问答 |
| **Vue3 前端** | 图谱渲染 + 对话界面 | D5 展示层 |

---

## 二、技术栈

| 层级 | 技术选型 | 说明 |
|------|---------|------|
| **前端** | Vue3 + ECharts + Element Plus | 图谱可视化 + 对话 UI |
| **后端** | Flask + Python 3.10+ | REST API |
| **图数据库** | Neo4j 5.x | 生物医学知识图谱 |
| **向量数据库** | FAISS | 文献向量检索 |
| **大模型** | DeepSeek API | 问答生成 |
| **RAG 框架** | LangChain | 检索 + 问答链 |
| **嵌入模型** | BGE-base-zh | 中英文混合嵌入 |
| **部署** | Docker Compose | 一键启动 |

---

## 三、数据源

| 数据集 | 用途 | 获取方式 |
|-------|------|---------|
| **Hetionet** | 知识图谱（药物+靶点+疾病+基因，11 类节点/24 类关系） | [het.io](https://het.io)，CC0 许可，直接下载 |
| **PubMed 摘要** | RAG 文献库 | `biopython` Entrez 模块 |

**数据规模**（小而全）：
- 图谱：3k-8k 节点 / 10k-30k 关系（Hetionet 子集）
- 文献：100-200 篇 PubMed 摘要

---

## 四、实施计划

### Week 1：知识图谱 + 可视化（~15h）

- [ ] 下载 Hetionet 数据并筛选子集（按疾病领域过滤）
- [ ] Neo4j 导入：节点（Compound/Gene/Disease/Pathway）+ 关系
- [ ] Flask API：实体查询、邻居查询
- [ ] Vue3 + ECharts 图谱可视化（限定 2-hop 子图）
- [ ] 搜索框（名称模糊查询）

**交付物**：可独立演示的图谱浏览器

### Week 2：RAG 问答 + 部署收尾（~15h）

- [ ] `biopython.Entrez` 爬取 PubMed 摘要（100-200 篇）
- [ ] FAISS 向量化（BGE-base-zh 嵌入）
- [ ] LangChain RAG 链：Retriever + Prompt（强制引用 PMID）+ DeepSeek API
- [ ] Flask `/chat` 接口 + Vue3 对话界面
- [ ] Docker Compose 一键启动（Neo4j + Flask + 前端）
- [ ] README 完善（架构图 + 快速开始）

**交付物**：完整可运行的 mini-D5 系统

---

## 五、目录结构

```
mini-d5/
├── README.md
├── docker-compose.yml
├── .env.example
├── backend/
│   ├── app.py
│   ├── requirements.txt
│   ├── api/
│   │   ├── graph.py            # 图谱查询
│   │   └── chat.py             # RAG 问答
│   └── services/
│       ├── neo4j_client.py
│       └── rag_chain.py
├── frontend/
│   ├── package.json
│   └── src/
│       └── views/
│           ├── GraphView.vue
│           └── ChatView.vue
├── data/
│   ├── fetch_hetionet.py
│   ├── fetch_pubmed.py
│   ├── build_graph.py
│   └── raw/
└── rag/
    ├── build_vectorstore.py
    └── prompts/
```

---

## 六、参考资料

- [Hetionet](https://het.io/) - 知识图谱数据源
- [PubMed E-utilities](https://www.ncbi.nlm.nih.gov/books/NBK25501/) - 文献获取
- [Neo4j Python Driver](https://neo4j.com/docs/python-manual/current/)
- [LangChain](https://python.langchain.com/)
- [DeepSeek API](https://platform.deepseek.com/)

---

> **mini-D5 = 知识图谱 + RAG 问答**，2 周轻量交付，展示对 D5 平台的理解。
