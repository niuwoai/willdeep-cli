# GLM-5 与 DeepSeek rc5 失败分析

GLM-5：43/60；DeepSeek原始41/60，独立故障补测3/5，组合44/60。组合不是一次完整重跑。版本与预算同时变化，不能归因全部代码改进。

分类依据官方FAIL_TO_PASS与PASS_TO_PASS裁判结果。超时、轮次上限和预算停止是执行约束，不能单独证明根因。未公开私有正文。

## GLM-5：17题未通过

|题目|证据分类|停止条件|关键失败测试|
|---|---|---|---|
|astropy__astropy-13398|目标修复不完整，并引入回归|max_turns|astropy/coordinates/tests/test_intermediate_transformations.py::test_itrs_topo_to_altaz_with_refraction；astropy/coordinates/tests/test_intermediate_transformations.py::test_itrs_topo_to_hadec_with_refraction；astropy/coordinates/tests/test_intermediate_transformations.py::test_gcrs_altaz_bothroutes[testframe0]；astropy/coordinates/tests/test_intermediate_transformations.py::test_gcrs_altaz_bothroutes[testframe1]|
|astropy__astropy-13977|目标修复不完整，并引入回归|unverified|astropy/units/tests/test_quantity_ufuncs.py::TestUfuncReturnsNotImplemented::TestBinaryUfuncs::test_full[empty-duck_quantity0-quantity0-add]；astropy/units/tests/test_quantity_ufuncs.py::TestUfuncReturnsNotImplemented::TestBinaryUfuncs::test_full[empty-duck_quantity0-quantity0-multiply]；astropy/units/tests/test_quantity_ufuncs.py::TestUfuncReturnsNotImplemented::TestBinaryUfuncs::test_basic[duck_quantity1-quantity0-add]；astropy/units/tests/test_quantity_ufuncs.py::TestUfuncReturnsNotImplemented::TestBinaryUfuncs::test_basic[duck_quantity1-quantity0-less]|
|django__django-10554|SQL排序表达式与查询输出列不匹配（日志确认）|max_turns||
|django__django-11265|目标行为未修复|unverified|test_with_exclude (filtered_relation.tests.FilteredRelationTests)|
|django__django-11885|目标修复成功，但引入原有测试回归|max_turns|test_large_delete (delete.tests.DeletionTests)；test_large_delete_related (delete.tests.DeletionTests)|
|django__django-14034|目标行为未修复|unverified|test_render_required_attributes (forms_tests.field_tests.test_multivaluefield.MultiValueFieldTest)|
|django__django-14725|上游调用故障，不能判定模型能力|agent_error||
|django__django-16032|裁判补丁执行阶段未通过，不能仅据该标记确定根因|max_turns||
|django__django-16145|目标修复成功，但引入原有测试回归|unverified|directory: django-admin builtin commands fail if settings file (from|
|django__django-16256|只修复部分目标用例|unverified|test_generic_async_acreate (generic_relations.tests.GenericRelationsTests)；test_generic_async_aget_or_create (generic_relations.tests.GenericRelationsTests)|
|matplotlib__matplotlib-26208|目标行为未修复|1200秒超时|lib/matplotlib/tests/test_axes.py::test_twin_units[x]；lib/matplotlib/tests/test_axes.py::test_twin_units[y]|
|pylint-dev__pylint-4551|目标行为未修复|unverified|tests/unittest_pyreverse_writer.py::test_dot_files[packages_No_Name.dot]；tests/unittest_pyreverse_writer.py::test_dot_files[classes_No_Name.dot]|
|scikit-learn__scikit-learn-26194|目标行为未修复|unverified|sklearn/metrics/tests/test_ranking.py::test_roc_curve_drop_intermediate；sklearn/metrics/tests/test_ranking.py::test_roc_curve_with_probablity_estimates[42]|
|sphinx-doc__sphinx-7462|目标行为未修复|unverified|tests/test_domain_py.py::test_parse_annotation；tests/test_pycode_ast.py::test_unparse[()-()]|
|sympy__sympy-12489|目标修复不完整，并引入回归|max_turns|test_Permutation_subclassing；test_josephus；test_ranking|
|sympy__sympy-18199|目标行为未修复|unverified|test_solve_modular|
|sympy__sympy-20438|目标行为未修复|unverified|test_Eq；test_issue_19378|

执行约束统计：超时1题；轮次上限5题；预算守卫停止0题。

## DeepSeek（故障补测组合）：16题未通过

|题目|证据分类|停止条件|关键失败测试|
|---|---|---|---|
|matplotlib__matplotlib-14623|没有提交补丁|1200秒超时||
|django__django-13297|目标修复成功，但引入原有测试回归|finished|test_template_params_warning (generic_views.test_base.DeprecationTests)|
|pylint-dev__pylint-4551|目标行为未修复|finished|tests/unittest_pyreverse_writer.py::test_dot_files[packages_No_Name.dot]；tests/unittest_pyreverse_writer.py::test_dot_files[classes_No_Name.dot]|
|sympy__sympy-20438|目标行为未修复|1200秒超时|test_Eq；test_issue_19378|
|sympy__sympy-11618|目标行为未修复|unverified|test_issue_11617|
|scikit-learn__scikit-learn-10908|没有提交补丁|1200秒超时||
|django__django-16256|只修复部分目标用例|unverified|test_generic_async_acreate (generic_relations.tests.GenericRelationsTests)；test_generic_async_aget_or_create (generic_relations.tests.GenericRelationsTests)|
|django__django-10554|SQL排序表达式与查询输出列不匹配（日志确认）|max_turns||
|django__django-13112|目标行为未修复|unresolved|test_reference_mixed_case_app_label (migrations.test_state.StateTests)|
|matplotlib__matplotlib-26208|没有提交补丁|1200秒超时||
|sphinx-doc__sphinx-7462|只修复部分目标用例|finished|tests/test_pycode_ast.py::test_unparse[()-()]|
|astropy__astropy-13398|目标修复不完整，并引入回归|1200秒超时|astropy/coordinates/tests/test_intermediate_transformations.py::test_itrs_topo_to_altaz_with_refraction；astropy/coordinates/tests/test_intermediate_transformations.py::test_itrs_topo_to_hadec_with_refraction；astropy/coordinates/tests/test_intermediate_transformations.py::test_gcrs_altaz_bothroutes[testframe0]；astropy/coordinates/tests/test_intermediate_transformations.py::test_gcrs_altaz_bothroutes[testframe1]|
|scikit-learn__scikit-learn-26194|目标行为未修复|finished|sklearn/metrics/tests/test_ranking.py::test_roc_curve_drop_intermediate；sklearn/metrics/tests/test_ranking.py::test_roc_curve_with_probablity_estimates[42]|
|sympy__sympy-18199|目标行为未修复|finished|test_solve_modular|
|django__django-16950|目标修复成功，但引入原有测试回归|1200秒超时|#24377 - Inlines with a model field default should ignore that default|
|django__django-14034|目标行为未修复|unverified|test_render_required_attributes (forms_tests.field_tests.test_multivaluefield.MultiValueFieldTest)|

执行约束统计：超时6题；轮次上限1题；预算守卫停止0题。

## 优先改进方向

双方共同失败10题：astropy__astropy-13398、django__django-10554、django__django-14034、django__django-16256、matplotlib__matplotlib-26208、pylint-dev__pylint-4551、scikit-learn__scikit-learn-26194、sphinx-doc__sphinx-7462、sympy__sympy-18199、sympy__sympy-20438。优先针对这些题做目标用例复现及原有测试回归验证。

- 目标测试未通过：先定位真正的行为入口，覆盖边界条件，避免只改表面输出。
- 原有测试回归：每次修复同时跑目标测试与受影响模块原有测试，禁止只看新用例变绿。
- 空补丁：在退出前验证git diff及提交物；将权限、测试环境和模型未形成修复分别记录。
- 超时或轮次上限：提前收集可用补丁、及时收尾；不等同于提高token预算就能解决。
- 基建：容器删除前保全工作树和索引状态，镜像准备与答题分开恢复，502单列。

完整逐题测试列表、修改文件和停止条件见 failures.json。
